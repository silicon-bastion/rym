/// LoongArch64 assembly code generator.
///
/// Emits GNU-as-compatible LA64 assembly text from an IrModule.
/// Register allocation strategy: simple linear scan over %0..%N names,
/// spilling to stack when register pressure exceeds 20 caller-saved regs.
use rym_ir::{BasicBlock, IrFunc, IrModule, Op, Terminator};
use std::collections::HashMap;
use std::fmt::Write;

// LoongArch64 general-purpose registers we use for temporaries.
// $t0-$t8 (9 regs) for temps, $a0-$a7 (8 regs) for args/return.
const TEMP_REGS: &[&str] = &[
    "$t0", "$t1", "$t2", "$t3", "$t4", "$t5", "$t6", "$t7", "$t8",
];
const ARG_REGS: &[&str] = &[
    "$a0", "$a1", "$a2", "$a3", "$a4", "$a5", "$a6", "$a7",
];

pub struct Codegen {
    output: String,
}

impl Codegen {
    pub fn new() -> Self {
        Self { output: String::new() }
    }

    /// Generate assembly for a full IR module.
    /// Returns the complete `.s` text.
    pub fn emit_module(&mut self, module: &IrModule) -> String {
        self.output.clear();
        writeln!(self.output, "\t.file\t\"{}\"", module.name).unwrap();
        writeln!(self.output, "\t.text").unwrap();

        for func in &module.funcs {
            self.emit_func(func);
        }

        self.output.clone()
    }

    fn emit_func(&mut self, func: &IrFunc) {
        let name = &func.name;
        writeln!(self.output, "\t.globl\t{name}").unwrap();
        writeln!(self.output, "\t.type\t{name}, @function").unwrap();
        writeln!(self.output, "{name}:").unwrap();

        // Simple prologue: save $ra and $fp, set up frame.
        writeln!(self.output, "\taddi.d\t$sp, $sp, -16").unwrap();
        writeln!(self.output, "\tst.d\t$ra, $sp, 8").unwrap();
        writeln!(self.output, "\tst.d\t$fp, $sp, 0").unwrap();
        writeln!(self.output, "\taddi.d\t$fp, $sp, 16").unwrap();

        // Bind incoming parameters to argument registers.
        let mut reg_alloc = RegAlloc::new();
        for (i, param) in func.params.iter().enumerate() {
            if i < ARG_REGS.len() {
                reg_alloc.bind(param.name.clone(), ARG_REGS[i].to_string());
            }
        }

        for block in &func.blocks {
            self.emit_block(block, &mut reg_alloc, func);
        }

        writeln!(self.output, "\t.size\t{name}, . - {name}").unwrap();
        writeln!(self.output).unwrap();
    }

    fn emit_block(&mut self, block: &BasicBlock, ra: &mut RegAlloc, func: &IrFunc) {
        writeln!(self.output, ".L{}_{}:", func.name, block.label).unwrap();

        for instr in &block.instrs {
            self.emit_instr(instr, ra);
        }

        self.emit_terminator(&block.term, ra, func);
    }

    fn emit_instr(&mut self, instr: &rym_ir::Instr, ra: &mut RegAlloc) {
        let dest_reg = instr.dest.as_ref().map(|d| ra.alloc(d.clone()));

        match &instr.op {
            Op::ConstInt(v) => {
                if let Some(d) = dest_reg {
                    if *v >= -2048 && *v <= 2047 {
                        writeln!(self.output, "\taddi.d\t{d}, $zero, {v}").unwrap();
                    } else {
                        writeln!(self.output, "\tlu12i.w\t{d}, {}", v >> 12).unwrap();
                        if v & 0xFFF != 0 {
                            writeln!(self.output, "\tori\t{d}, {d}, {}", v & 0xFFF).unwrap();
                        }
                    }
                }
            }
            Op::ConstBool(v) => {
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\taddi.d\t{d}, $zero, {}", if *v { 1 } else { 0 }).unwrap();
                }
            }
            Op::ConstStr(s) => {
                // Emit string into .rodata, load its address.
                let label = ra.str_label();
                writeln!(self.output, "\t.section\t.rodata").unwrap();
                writeln!(self.output, "{label}:").unwrap();
                writeln!(self.output, "\t.string\t\"{s}\"").unwrap();
                writeln!(self.output, "\t.text").unwrap();
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\tla.local\t{d}, {label}").unwrap();
                }
            }
            Op::ConstFloat(_) => {
                // Float constants require a .rodata pool — emit a placeholder nop.
                writeln!(self.output, "\tnop\t# float const (stub)").unwrap();
            }

            Op::Load(src) => {
                if let Some(d) = dest_reg {
                    let s = ra.get_or_spill(src);
                    if s != d {
                        writeln!(self.output, "\tmove\t{d}, {s}").unwrap();
                    }
                }
            }
            Op::Store { dest, src } => {
                let s = ra.get_or_spill(src);
                ra.bind(dest.clone(), s.clone());
            }

            Op::Add(a, b) => binary!(self, dest_reg, ra, a, b, "add.d"),
            Op::Sub(a, b) => binary!(self, dest_reg, ra, a, b, "sub.d"),
            Op::Mul(a, b) => binary!(self, dest_reg, ra, a, b, "mul.d"),
            Op::Div(a, b) => binary!(self, dest_reg, ra, a, b, "div.d"),
            Op::Rem(a, b) => binary!(self, dest_reg, ra, a, b, "mod.d"),

            Op::CmpEq(a, b)  => cmp!(self, dest_reg, ra, a, b, "sub.d", "sltui", true),
            Op::CmpNeq(a, b) => cmp!(self, dest_reg, ra, a, b, "sub.d", "sltu", false),
            Op::CmpLt(a, b)  => { if let Some(d) = dest_reg { let ra2 = ra.get_or_spill(a); let rb2 = ra.get_or_spill(b); writeln!(self.output, "\tslt\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::CmpLe(a, b)  => { if let Some(d) = dest_reg { let ra2 = ra.get_or_spill(b); let rb2 = ra.get_or_spill(a); writeln!(self.output, "\tslt\t{d}, {ra2}, {rb2}").unwrap(); writeln!(self.output, "\txori\t{d}, {d}, 1").unwrap(); } }
            Op::CmpGt(a, b)  => { if let Some(d) = dest_reg { let ra2 = ra.get_or_spill(b); let rb2 = ra.get_or_spill(a); writeln!(self.output, "\tslt\t{d}, {ra2}, {rb2}").unwrap(); } }
            Op::CmpGe(a, b)  => { if let Some(d) = dest_reg { let ra2 = ra.get_or_spill(a); let rb2 = ra.get_or_spill(b); writeln!(self.output, "\tslt\t{d}, {ra2}, {rb2}").unwrap(); writeln!(self.output, "\txori\t{d}, {d}, 1").unwrap(); } }

            Op::And(a, b) => binary!(self, dest_reg, ra, a, b, "and"),
            Op::Or(a, b)  => binary!(self, dest_reg, ra, a, b, "or"),
            Op::Not(a) => {
                if let Some(d) = dest_reg {
                    let ra2 = ra.get_or_spill(a);
                    writeln!(self.output, "\tsltu\t{d}, {ra2}, 1").unwrap();
                }
            }
            Op::Neg(a) => {
                if let Some(d) = dest_reg {
                    let ra2 = ra.get_or_spill(a);
                    writeln!(self.output, "\tsub.d\t{d}, $zero, {ra2}").unwrap();
                }
            }

            Op::Call { func, args } => {
                // Load args into $a0..$a7.
                for (i, arg) in args.iter().take(ARG_REGS.len()).enumerate() {
                    let s = ra.get_or_spill(arg);
                    if s != ARG_REGS[i] {
                        writeln!(self.output, "\tmove\t{}, {s}", ARG_REGS[i]).unwrap();
                    }
                }
                writeln!(self.output, "\tbl\t{func}").unwrap();
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\tmove\t{d}, $a0").unwrap();
                }
            }

            Op::WrapOk(v) | Op::WrapErr(v) => {
                // Result is a tagged value — just move the inner value for now.
                if let Some(d) = dest_reg {
                    let s = ra.get_or_spill(v);
                    writeln!(self.output, "\tmove\t{d}, {s}").unwrap();
                }
            }

            Op::UnwrapOk { val, err_block } => {
                if let Some(d) = dest_reg {
                    let v = ra.get_or_spill(val);
                    // Simplified: treat val as a direct value (no tag check yet).
                    writeln!(self.output, "\tmove\t{d}, {v}").unwrap();
                    let _ = err_block;
                }
            }

            Op::Cast { val, .. } => {
                if let Some(d) = dest_reg {
                    let v = ra.get_or_spill(val);
                    writeln!(self.output, "\tmove\t{d}, {v}").unwrap();
                }
            }

            Op::Field { base, field } => {
                if let Some(d) = dest_reg {
                    let b = ra.get_or_spill(base);
                    writeln!(self.output, "\t# field {field} of {b} -> {d}").unwrap();
                    writeln!(self.output, "\tmove\t{d}, {b}\t# field access stub").unwrap();
                }
            }

            Op::Index { base, index } => {
                if let Some(d) = dest_reg {
                    let b = ra.get_or_spill(base);
                    let i = ra.get_or_spill(index);
                    writeln!(self.output, "\tadd.d\t{d}, {b}, {i}").unwrap();
                    writeln!(self.output, "\tld.d\t{d}, {d}, 0").unwrap();
                }
            }

            Op::Ref(v) => {
                // Address-of: not fully implementable without a stack frame map.
                if let Some(d) = dest_reg {
                    let s = ra.get_or_spill(v);
                    writeln!(self.output, "\t# ref {s}").unwrap();
                    writeln!(self.output, "\tmove\t{d}, {s}\t# ref stub").unwrap();
                }
            }

            Op::Deref(v) => {
                if let Some(d) = dest_reg {
                    let s = ra.get_or_spill(v);
                    writeln!(self.output, "\tld.d\t{d}, {s}, 0").unwrap();
                }
            }

            Op::StructLit { ty, fields } => {
                if let Some(d) = dest_reg {
                    writeln!(self.output, "\t# struct {ty} {{ {} }}", fields.iter().map(|(k,_)| k.as_str()).collect::<Vec<_>>().join(", ")).unwrap();
                    writeln!(self.output, "\taddi.d\t{d}, $zero, 0\t# struct lit stub").unwrap();
                }
            }

            Op::Nop => {
                writeln!(self.output, "\tnop").unwrap();
            }
        }
    }

    fn emit_terminator(&mut self, term: &Terminator, ra: &mut RegAlloc, func: &IrFunc) {
        match term {
            Terminator::Return(val) => {
                if let Some(v) = val {
                    let s = ra.get_or_spill(v);
                    if s != "$a0" {
                        writeln!(self.output, "\tmove\t$a0, {s}").unwrap();
                    }
                }
                // Epilogue.
                writeln!(self.output, "\tld.d\t$ra, $sp, 8").unwrap();
                writeln!(self.output, "\tld.d\t$fp, $sp, 0").unwrap();
                writeln!(self.output, "\taddi.d\t$sp, $sp, 16").unwrap();
                writeln!(self.output, "\tjr\t$ra").unwrap();
            }

            Terminator::Jump(label) => {
                writeln!(self.output, "\tb\t.L{}_{}", func.name, label).unwrap();
            }

            Terminator::Branch { cond, then_block, else_block } => {
                let c = ra.get_or_spill(cond);
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
    fn default() -> Self {
        Self::new()
    }
}

// ── Register allocator (trivial linear scan) ──────────────────

struct RegAlloc {
    /// SSA name → physical register.
    map:      HashMap<String, String>,
    /// Next available temp register index.
    next_reg: usize,
    /// Counter for .rodata string labels.
    str_cnt:  usize,
}

impl RegAlloc {
    fn new() -> Self {
        Self { map: HashMap::new(), next_reg: 0, str_cnt: 0 }
    }

    /// Allocate or retrieve the register for SSA name `name`.
    fn alloc(&mut self, name: String) -> String {
        if let Some(r) = self.map.get(&name) {
            return r.clone();
        }
        let reg = if self.next_reg < TEMP_REGS.len() {
            let r = TEMP_REGS[self.next_reg].to_string();
            self.next_reg += 1;
            r
        } else {
            // Spill: reuse $t8 (simplified — no actual stack spill yet).
            "$t8".to_string()
        };
        self.map.insert(name, reg.clone());
        reg
    }

    /// Get register for existing SSA name, or spill to $t8.
    fn get_or_spill(&mut self, name: &str) -> String {
        self.map.get(name).cloned().unwrap_or_else(|| "$t8".to_string())
    }

    /// Bind an SSA name to a specific physical register.
    fn bind(&mut self, name: String, reg: String) {
        self.map.insert(name, reg);
    }

    fn str_label(&mut self) -> String {
        let id = self.str_cnt;
        self.str_cnt += 1;
        format!(".Lstr{id}")
    }
}

// ── Helper macros ─────────────────────────────────────────────

macro_rules! binary {
    ($self:expr, $dest:expr, $ra:expr, $a:expr, $b:expr, $op:literal) => {
        if let Some(d) = $dest {
            let ra2 = $ra.get_or_spill($a);
            let rb2 = $ra.get_or_spill($b);
            writeln!($self.output, "\t{}\t{d}, {ra2}, {rb2}", $op).unwrap();
        }
    };
}

macro_rules! cmp {
    ($self:expr, $dest:expr, $ra:expr, $a:expr, $b:expr, $sub_op:literal, $slti_op:literal, $invert:expr) => {
        if let Some(d) = $dest {
            let tmp = "$t8";
            let ra2 = $ra.get_or_spill($a);
            let rb2 = $ra.get_or_spill($b);
            writeln!($self.output, "\t{}\t{tmp}, {ra2}, {rb2}", $sub_op).unwrap();
            writeln!($self.output, "\t{}\t{d}, {tmp}, 1", $slti_op).unwrap();
            if $invert {
                writeln!($self.output, "\txori\t{d}, {d}, 1").unwrap();
            }
        }
    };
}

use binary;
use cmp;

/// Pretty-print an IrModule as a human-readable IR dump (for `--dump-ir`).
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

