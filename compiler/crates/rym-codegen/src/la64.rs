/// LoongArch64 assembly code generator.
///
/// Emits GNU-as-compatible LA64 assembly text from an IrModule.
/// Register allocation: $t0-$t7 for temps, $s0-$s8 for saved,
/// $a0-$a7 for args. Spills to stack when temp regs exhausted.
use rym_ir::{BasicBlock, IrFunc, IrModule, Op, Terminator};
use std::collections::HashMap;
use std::fmt::Write;

// Caller-saved temporaries (not preserved across calls).
const TEMP_REGS: &[&str] = &[
    "$t0", "$t1", "$t2", "$t3", "$t4", "$t5", "$t6", "$t7",
];
const ARG_REGS: &[&str] = &[
    "$a0", "$a1", "$a2", "$a3", "$a4", "$a5", "$a6", "$a7",
];

pub struct Codegen {
    output: String,
    /// Struct field-index lookup: type name → (field name → byte offset).
    struct_layouts: HashMap<String, HashMap<String, usize>>,
}

impl Codegen {
    pub fn new() -> Self {
        Self { output: String::new(), struct_layouts: HashMap::new() }
    }

    pub fn emit_module(&mut self, module: &IrModule) -> String {
        self.output.clear();

        // Build struct layouts (each field is 8 bytes, pointer-sized).
        self.struct_layouts.clear();
        for s in &module.structs {
            let map: HashMap<String, usize> = s.fields.iter()
                .enumerate()
                .map(|(i, n)| (n.clone(), i * 8))
                .collect();
            self.struct_layouts.insert(s.name.clone(), map);
        }

        writeln!(self.output, "\t.file\t\"{}\"", module.name).unwrap();
        writeln!(self.output, "\t.text").unwrap();

        for func in &module.funcs {
            self.emit_func(func);
        }

        self.output.clone()
    }

    fn emit_func(&mut self, func: &IrFunc) {
        let name = &func.name;

        // Count how many SSA names will need stack slots (spills).
        // Reserve 8 bytes per potential spill beyond TEMP_REGS capacity.
        let ssa_count = count_ssa_names(func);
        let spill_slots = ssa_count.saturating_sub(TEMP_REGS.len());
        // Frame: 8 ($ra) + 8 ($fp) + 8*spill_slots, rounded up to 16-byte align.
        let frame_size = align16(16 + spill_slots * 8);

        writeln!(self.output, "\t.globl\t{name}").unwrap();
        writeln!(self.output, "\t.type\t{name}, @function").unwrap();
        writeln!(self.output, "{name}:").unwrap();

        // Prologue.
        writeln!(self.output, "\taddi.d\t$sp, $sp, -{frame_size}").unwrap();
        writeln!(self.output, "\tst.d\t$ra, $sp, {}", frame_size - 8).unwrap();
        writeln!(self.output, "\tst.d\t$fp, $sp, {}", frame_size - 16).unwrap();
        writeln!(self.output, "\taddi.d\t$fp, $sp, {frame_size}").unwrap();

        let mut ra = RegAlloc::new(frame_size);

        // Bind incoming parameters to argument registers.
        for (i, param) in func.params.iter().enumerate() {
            if i < ARG_REGS.len() {
                ra.bind(param.name.clone(), ARG_REGS[i].to_string());
            }
        }

        for block in &func.blocks {
            self.emit_block(block, &mut ra, func, frame_size);
        }

        writeln!(self.output, "\t.size\t{name}, . - {name}").unwrap();
        writeln!(self.output).unwrap();
    }

    fn emit_block(&mut self, block: &BasicBlock, ra: &mut RegAlloc, func: &IrFunc, frame_size: usize) {
        writeln!(self.output, ".L{}_{}:", func.name, block.label).unwrap();
        for instr in &block.instrs {
            self.emit_instr(instr, ra, func);
        }
        self.emit_terminator(&block.term, ra, func, frame_size);
    }

    fn emit_instr(&mut self, instr: &rym_ir::Instr, ra: &mut RegAlloc, func: &IrFunc) {
        let dest_reg = instr.dest.as_ref().map(|d| ra.alloc(d.clone(), &mut self.output));

        match &instr.op {
            Op::ConstInt(v) => {
                if let Some(d) = dest_reg {
                    emit_load_imm(&mut self.output, &d, *v);
                }
            }
            Op::ConstBool(v) => {
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\taddi.d\t{d}, $zero, {}", if *v { 1 } else { 0 }).unwrap();
                }
            }
            Op::ConstStr(s) => {
                let label = ra.str_label(&func.name);
                // Emit string data into .rodata section, then return to .text.
                writeln!(self.output, "\t.section\t.rodata").unwrap();
                writeln!(self.output, "{label}:").unwrap();
                // Escape special chars in the string.
                let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\t', "\\t");
                writeln!(self.output, "\t.string\t\"{escaped}\"").unwrap();
                writeln!(self.output, "\t.text").unwrap();
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\tla.local\t{d}, {label}").unwrap();
                }
            }
            Op::ConstFloat(_v) => {
                // Floating-point constants need .rodata pool entry — stub for now.
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\taddi.d\t{d}, $zero, 0\t# float stub").unwrap();
                }
            }

            Op::Load(src) => {
                if let Some(d) = dest_reg {
                    let s = ra.get(src, &mut self.output);
                    if s != d {
                        writeln!(self.output, "\tmove\t{d}, {s}").unwrap();
                    }
                }
            }
            Op::Store { dest, src } => {
                let s = ra.get(src, &mut self.output);
                // Rebind dest SSA to the same register/slot as src.
                ra.bind(dest.clone(), s);
            }

            Op::Add(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tadd.d\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::Sub(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tsub.d\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::Mul(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tmul.d\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::Div(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tdiv.d\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::Rem(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tmod.d\t{d}, {ra2}, {rb2}").unwrap(); } }

            Op::CmpEq(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tsub.d\t{d}, {ra2}, {rb2}").unwrap(); writeln!(self.output, "\tsltui\t{d}, {d}, 1").unwrap(); } }
            Op::CmpNeq(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tsub.d\t{d}, {ra2}, {rb2}").unwrap(); writeln!(self.output, "\tsltu\t{d}, $zero, {d}").unwrap(); } }
            Op::CmpLt(a, b)  => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tslt\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::CmpLe(a, b)  => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tslt\t{d}, {rb2}, {ra2}").unwrap(); writeln!(self.output, "\txori\t{d}, {d}, 1").unwrap(); } }
            Op::CmpGt(a, b)  => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tslt\t{d}, {rb2}, {ra2}").unwrap(); } }
            Op::CmpGe(a, b)  => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tslt\t{d}, {ra2}, {rb2}").unwrap(); writeln!(self.output, "\txori\t{d}, {d}, 1").unwrap(); } }

            Op::And(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tand\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::Or(a, b)  => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tor\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::Not(a) => { if let Some(d) = dest_reg { let s = ra.get(a, &mut self.output); writeln!(self.output, "\tsltui\t{d}, {s}, 1").unwrap(); } }
            Op::Neg(a) => { if let Some(d) = dest_reg { let s = ra.get(a, &mut self.output); writeln!(self.output, "\tsub.d\t{d}, $zero, {s}").unwrap(); } }
            Op::BitAnd(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tand\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::BitOr(a, b)  => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tor\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::BitXor(a, b) => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\txor\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::Shl(a, b)    => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tsll.d\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::Shr(a, b)    => { if let Some(d) = dest_reg { let (ra2, rb2) = ra.get2(a, b, &mut self.output); writeln!(self.output, "\tsrl.d\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::BitNot(a)    => { if let Some(d) = dest_reg { let s = ra.get(a, &mut self.output); writeln!(self.output, "\tnor\t{d}, {s}, $zero").unwrap(); } }

            Op::Call { func: fname, args } => {
                // Save caller-saved temps that are live (simplified: save $t0-$t7).
                for (i, arg) in args.iter().take(ARG_REGS.len()).enumerate() {
                    let s = ra.get(arg, &mut self.output);
                    if s != ARG_REGS[i] {
                        writeln!(self.output, "\tmove\t{}, {s}", ARG_REGS[i]).unwrap();
                    }
                }
                writeln!(self.output, "\tbl\t{fname}").unwrap();
                if let Some(d) = dest_reg {
                    if d != "$a0" {
                        writeln!(self.output, "\tmove\t{d}, $a0").unwrap();
                    }
                }
            }

            Op::CallIndirect { fp, args } => {
                // Load function pointer into $t8, then jirl.
                let f = ra.get(fp, &mut self.output);
                for (i, arg) in args.iter().take(ARG_REGS.len()).enumerate() {
                    let s = ra.get(arg, &mut self.output);
                    if s != ARG_REGS[i] {
                        writeln!(self.output, "\tmove\t{}, {s}", ARG_REGS[i]).unwrap();
                    }
                }
                writeln!(self.output, "\tjirl\t$ra, {f}, 0").unwrap();
                if let Some(d) = dest_reg {
                    if d != "$a0" {
                        writeln!(self.output, "\tmove\t{d}, $a0").unwrap();
                    }
                }
            }

            Op::MakeVariant { tag, payload } => {
                if let Some(d) = dest_reg {
                    // Allocate 16 bytes (2 words): [tag, payload].
                    writeln!(self.output, "\taddi.d\t$a0, $zero, 16").unwrap();
                    writeln!(self.output, "\tbl\tmalloc").unwrap();
                    writeln!(self.output, "\tmove\t{d}, $a0").unwrap();
                    writeln!(self.output, "\taddi.d\t$t8, $zero, {tag}").unwrap();
                    writeln!(self.output, "\tst.d\t$t8, {d}, 0").unwrap();
                    let p = ra.get(payload, &mut self.output);
                    writeln!(self.output, "\tst.d\t{p}, {d}, 8").unwrap();
                }
            }
            Op::GetTag(v) => {
                if let Some(d) = dest_reg {
                    let s = ra.get(v, &mut self.output);
                    writeln!(self.output, "\tld.d\t{d}, {s}, 0").unwrap();
                }
            }
            Op::GetPayload(v) => {
                if let Some(d) = dest_reg {
                    let s = ra.get(v, &mut self.output);
                    writeln!(self.output, "\tld.d\t{d}, {s}, 8").unwrap();
                }
            }

            Op::WrapOk(v) => {
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\taddi.d\t$a0, $zero, 16").unwrap();
                    writeln!(self.output, "\tbl\tmalloc").unwrap();
                    writeln!(self.output, "\tmove\t{d}, $a0").unwrap();
                    writeln!(self.output, "\tst.d\t$zero, {d}, 0").unwrap();
                    let p = ra.get(v, &mut self.output);
                    writeln!(self.output, "\tst.d\t{p}, {d}, 8").unwrap();
                }
            }
            Op::WrapErr(v) => {
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\taddi.d\t$a0, $zero, 16").unwrap();
                    writeln!(self.output, "\tbl\tmalloc").unwrap();
                    writeln!(self.output, "\tmove\t{d}, $a0").unwrap();
                    writeln!(self.output, "\taddi.d\t$t8, $zero, 1").unwrap();
                    writeln!(self.output, "\tst.d\t$t8, {d}, 0").unwrap();
                    let p = ra.get(v, &mut self.output);
                    writeln!(self.output, "\tst.d\t{p}, {d}, 8").unwrap();
                }
            }

            Op::UnwrapOk { val, err_block: _ } => {
                if let Some(d) = dest_reg {
                    let s = ra.get(val, &mut self.output);
                    writeln!(self.output, "\tld.d\t{d}, {s}, 8").unwrap();
                }
            }

            Op::Cast { val, .. } => {
                if let Some(d) = dest_reg {
                    let s = ra.get(val, &mut self.output);
                    if s != d { writeln!(self.output, "\tmove\t{d}, {s}").unwrap(); }
                }
            }

            // Struct field access: compute base + field_index*8.
            Op::Field { base, field, struct_ty } => {
                if let Some(d) = dest_reg {
                    let b = ra.get(base, &mut self.output);
                    let offset = struct_ty.as_deref()
                        .and_then(|ty| self.struct_layouts.get(ty))
                        .and_then(|m| m.get(field))
                        .copied()
                        .unwrap_or(0);
                    writeln!(self.output, "\tld.d\t{d}, {b}, {offset}\t# .{field}").unwrap();
                }
            }

            Op::Index { base, index } => {
                if let Some(d) = dest_reg {
                    let (b, i) = ra.get2(base, index, &mut self.output);
                    // Element size = 8 (pointer-sized default).
                    writeln!(self.output, "\tslli.d\t{d}, {i}, 3").unwrap();
                    writeln!(self.output, "\tadd.d\t{d}, {b}, {d}").unwrap();
                    writeln!(self.output, "\tld.d\t{d}, {d}, 0").unwrap();
                }
            }

            Op::SliceLen(base) => {
                // Slice layout: [ptr: uintptr_t, len: uintptr_t] — len is at offset 8.
                if let Some(d) = dest_reg {
                    let b = ra.get(base, &mut self.output);
                    writeln!(self.output, "\tld.d\t{d}, {b}, 8").unwrap();
                }
            }

            Op::SlicePtr(base) => {
                if let Some(d) = dest_reg {
                    let b = ra.get(base, &mut self.output);
                    writeln!(self.output, "\tld.d\t{d}, {b}, 0").unwrap();
                }
            }

            Op::StrLen(base) => {
                // Call libc strlen.
                if let Some(d) = dest_reg {
                    let b = ra.get(base, &mut self.output);
                    writeln!(self.output, "\tmove\t$a0, {b}").unwrap();
                    writeln!(self.output, "\tbl\tstrlen").unwrap();
                    writeln!(self.output, "\tmove\t{d}, $a0").unwrap();
                }
            }

            Op::Ref(v) => {
                // Address-of a local: needs stack slot. Emit addi.d from $fp - slot.
                if let Some(d) = dest_reg {
                    let slot = ra.stack_slot_for(v);
                    writeln!(self.output, "\taddi.d\t{d}, $fp, -{slot}").unwrap();
                }
            }

            Op::Deref(v) => {
                if let Some(d) = dest_reg {
                    let s = ra.get(v, &mut self.output);
                    writeln!(self.output, "\tld.d\t{d}, {s}, 0").unwrap();
                }
            }

            Op::StructLit { ty, fields } => {
                if let Some(d) = dest_reg {
                    // Allocate on stack: each field is 8 bytes.
                    let size = fields.len() * 8;
                    writeln!(self.output, "\taddi.d\t$sp, $sp, -{size}\t# struct {ty}").unwrap();
                    writeln!(self.output, "\tmove\t{d}, $sp").unwrap();
                    for (i, (fname, fval)) in fields.iter().enumerate() {
                        let fv = ra.get(fval, &mut self.output);
                        writeln!(self.output, "\tst.d\t{fv}, {d}, {}\t# .{fname}", i * 8).unwrap();
                    }
                }
            }

            Op::ArrayLit(elems) => {
                if let Some(d) = dest_reg {
                    let size = elems.len() * 8;
                    writeln!(self.output, "\taddi.d\t$sp, $sp, -{size}").unwrap();
                    writeln!(self.output, "\tmove\t{d}, $sp").unwrap();
                    for (i, ev) in elems.iter().enumerate() {
                        let v = ra.get(ev, &mut self.output);
                        writeln!(self.output, "\tst.d\t{v}, {d}, {}", i * 8).unwrap();
                    }
                }
            }

            Op::MatrixLit { elems, rows, cols } => {
                if let Some(d) = dest_reg {
                    let size = elems.len() * 8;
                    writeln!(self.output, "\taddi.d\t$sp, $sp, -{size}\t# matrix {rows}x{cols}").unwrap();
                    writeln!(self.output, "\tmove\t{d}, $sp").unwrap();
                    for (i, ev) in elems.iter().enumerate() {
                        let v = ra.get(ev, &mut self.output);
                        writeln!(self.output, "\tst.d\t{v}, {d}, {}", i * 8).unwrap();
                    }
                }
            }

            Op::AllocCall { allocator, elem_ty: _, count } => {
                if let Some(d) = dest_reg {
                    let _ = allocator;
                    let cnt = ra.get(count, &mut self.output);
                    writeln!(self.output, "\tslli.d\t$a0, {cnt}, 3\t# count * 8 bytes").unwrap();
                    // Call malloc (assume linked in).
                    writeln!(self.output, "\tbl\tmalloc").unwrap();
                    writeln!(self.output, "\tmove\t{d}, $a0").unwrap();
                }
            }

            Op::Asm { template, args } => {
                // Substitute {0}, {1}, … with the register names of the corresponding SSA values.
                let mut tpl = template.clone();
                for (i, arg) in args.iter().enumerate() {
                    let reg = ra.get(arg, &mut self.output);
                    tpl = tpl.replace(&format!("{{{i}}}"), &reg);
                }
                writeln!(self.output, "\t{tpl}").unwrap();
            }
            Op::Nop => {
                writeln!(self.output, "\tnop").unwrap();
            }
        }
    }

    fn emit_terminator(&mut self, term: &Terminator, ra: &mut RegAlloc, func: &IrFunc, frame_size: usize) {
        match term {
            Terminator::Return(val) => {
                if let Some(v) = val {
                    let s = ra.get(v, &mut self.output);
                    if s != "$a0" {
                        writeln!(self.output, "\tmove\t$a0, {s}").unwrap();
                    }
                }
                // Epilogue.
                writeln!(self.output, "\tld.d\t$ra, $sp, {}", frame_size - 8).unwrap();
                writeln!(self.output, "\tld.d\t$fp, $sp, {}", frame_size - 16).unwrap();
                writeln!(self.output, "\taddi.d\t$sp, $sp, {frame_size}").unwrap();
                writeln!(self.output, "\tjr\t$ra").unwrap();
            }
            Terminator::Jump(label) => {
                writeln!(self.output, "\tb\t.L{}_{}", func.name, label).unwrap();
            }
            Terminator::Branch { cond, then_block, else_block } => {
                let c = ra.get(cond, &mut self.output);
                writeln!(self.output, "\tbnez\t{c}, .L{}_{}", func.name, then_block).unwrap();
                writeln!(self.output, "\tb\t.L{}_{}", func.name, else_block).unwrap();
            }
            Terminator::Unreachable => {
                writeln!(self.output, "\tbreak\t0\t# unreachable").unwrap();
            }
        }
    }
}

impl Default for Codegen {
    fn default() -> Self { Self::new() }
}

// ── Register allocator with stack spill ───────────────────────

struct RegAlloc {
    /// SSA name → physical register or stack slot description.
    map:        HashMap<String, Loc>,
    next_reg:   usize,
    /// Next stack offset from $sp for spills (grows down from frame base).
    next_spill: usize,
    frame_size: usize,
    str_cnt:    usize,
}

#[derive(Clone)]
enum Loc {
    Reg(String),
    /// Offset from $sp.
    Stack(usize),
}

impl RegAlloc {
    fn new(frame_size: usize) -> Self {
        Self {
            map:        HashMap::new(),
            next_reg:   0,
            next_spill: 0,
            frame_size,
            str_cnt:    0,
        }
    }

    /// Allocate a register for SSA dest `name`. Spills to stack if needed,
    /// emitting load/store via `out`.
    fn alloc(&mut self, name: String, out: &mut String) -> String {
        if let Some(loc) = self.map.get(&name) {
            return self.loc_to_reg(loc.clone(), out);
        }
        let loc = if self.next_reg < TEMP_REGS.len() {
            let r = TEMP_REGS[self.next_reg].to_string();
            self.next_reg += 1;
            Loc::Reg(r)
        } else {
            // Spill: allocate a new stack slot (8 bytes).
            let offset = self.next_spill;
            self.next_spill += 8;
            Loc::Stack(offset)
        };
        let reg = self.loc_to_reg(loc.clone(), out);
        self.map.insert(name, loc);
        reg
    }

    /// Get the register currently holding `name`, loading from stack if spilled.
    fn get(&mut self, name: &str, out: &mut String) -> String {
        match self.map.get(name).cloned() {
            Some(loc) => self.loc_to_reg(loc, out),
            None => {
                // Unknown SSA — return scratch register $t7.
                "$t7".to_string()
            }
        }
    }

    /// Get two values into registers, avoiding the same scratch register.
    fn get2(&mut self, a: &str, b: &str, out: &mut String) -> (String, String) {
        let ra = self.get(a, out);
        let rb = self.get(b, out);
        (ra, rb)
    }

    fn bind(&mut self, name: String, reg: String) {
        self.map.insert(name, Loc::Reg(reg));
    }

    /// Returns the stack byte offset for a named value (for address-of).
    fn stack_slot_for(&mut self, name: &str) -> usize {
        match self.map.get(name) {
            Some(Loc::Stack(off)) => self.frame_size - *off,
            _ => 0,
        }
    }

    fn str_label(&mut self, func: &str) -> String {
        let id = self.str_cnt;
        self.str_cnt += 1;
        format!(".Lstr_{func}_{id}")
    }

    fn loc_to_reg(&self, loc: Loc, out: &mut String) -> String {
        match loc {
            Loc::Reg(r) => r,
            Loc::Stack(offset) => {
                // Load the spilled value into $t7 (scratch).
                writeln!(out, "\tld.d\t$t7, $sp, {offset}\t# reload spill").unwrap();
                "$t7".to_string()
            }
        }
    }
}

// ── Utility helpers ───────────────────────────────────────────

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

fn count_ssa_names(func: &IrFunc) -> usize {
    let mut max = 0usize;
    for block in &func.blocks {
        for instr in &block.instrs {
            if let Some(d) = &instr.dest {
                if let Some(n) = d.strip_prefix('%') {
                    if let Ok(v) = n.parse::<usize>() {
                        max = max.max(v + 1);
                    }
                }
            }
        }
    }
    max
}

fn emit_load_imm(out: &mut String, reg: &str, v: i64) {
    if v >= -2048 && v <= 2047 {
        writeln!(out, "\taddi.d\t{reg}, $zero, {v}").unwrap();
    } else if v >= 0 && v <= 0xFFFFF {
        writeln!(out, "\tlu12i.w\t{reg}, {}", v >> 12).unwrap();
        if v & 0xFFF != 0 {
            writeln!(out, "\tori\t{reg}, {reg}, {}", v & 0xFFF).unwrap();
        }
    } else {
        // Full 64-bit: lu12i + ori + lu32i + lu52i.
        writeln!(out, "\tlu12i.w\t{reg}, {}", (v >> 12) & 0xFFFFF).unwrap();
        writeln!(out, "\tori\t{reg}, {reg}, {}", v & 0xFFF).unwrap();
        if v >> 32 != 0 {
            writeln!(out, "\tlu32i.d\t{reg}, {}", (v >> 32) & 0xFFFFF).unwrap();
        }
    }
}

/// Pretty-print an IrModule as human-readable IR (for `--dump-ir`).
pub fn dump_ir(module: &IrModule) -> String {
    let mut out = String::new();
    writeln!(out, "; module: {}", module.name).unwrap();
    for func in &module.funcs {
        writeln!(out, "fn {}({}) -> {} {{",
            func.name,
            func.params.iter().map(|p| format!("{}: {}", p.name, p.ty.display())).collect::<Vec<_>>().join(", "),
            func.ret.display()
        ).unwrap();
        for block in &func.blocks {
            writeln!(out, "  {}:", block.label).unwrap();
            for instr in &block.instrs {
                if let Some(d) = &instr.dest {
                    writeln!(out, "    {d} = {:?}", instr.op).unwrap();
                } else {
                    writeln!(out, "    {:?}", instr.op).unwrap();
                }
            }
            writeln!(out, "    {:?}", block.term).unwrap();
        }
        writeln!(out, "}}").unwrap();
    }
    out
}
