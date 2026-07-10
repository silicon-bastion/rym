use clap::Parser;
use miette::{IntoDiagnostic, Result};
use rym_lexer::Lexer;
use rym_parser::Parser as RymParser;
use rym_sema::TyChecker;
use rym_ir::lower::Lowerer;
use rym_codegen::{Codegen, CCodegen, la64::dump_ir};
use rym_ast::{SourceFile, item::ItemKind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// rymc — the Rym language bootstrap compiler
#[derive(Parser)]
#[command(name = "rymc", version, about = "Rym language compiler")]
struct Cli {
    /// Source file to compile
    input: PathBuf,

    /// Output file (default: same name as input without extension)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Compilation target: c (default, any platform) or la64 (LoongArch64)
    #[arg(long, default_value = "c")]
    target: String,

    /// Only emit generated code, do not compile/link
    #[arg(long)]
    emit_only: bool,

    /// Dump token stream and exit
    #[arg(long)]
    dump_tokens: bool,

    /// Dump AST and exit
    #[arg(long)]
    dump_ast: bool,

    /// Dump IR and exit
    #[arg(long)]
    dump_ir: bool,

    /// Path to the Rym runtime start.s (la64 target only)
    #[arg(long)]
    runtime: Option<PathBuf>,

    /// C compiler to use (default: cc)
    #[arg(long, default_value = "cc")]
    cc: String,

    /// Assembler binary (la64 target only, default: as)
    #[arg(long, default_value = "as")]
    assembler: String,

    /// Linker binary (la64 target only, default: ld)
    #[arg(long, default_value = "ld")]
    linker: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let src = std::fs::read_to_string(&cli.input).into_diagnostic()?;
    let module_name = cli.input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module")
        .to_string();

    // ── Phase 1: Lex ─────────────────────────────────────────
    let tokens = Lexer::new(&src).tokenize()
        .map_err(|e| miette::miette!("lex error: {e}"))?;

    if cli.dump_tokens {
        for tok in &tokens { println!("{:?}", tok); }
        return Ok(());
    }

    // ── Phase 2: Parse ───────────────────────────────────────
    let mut ast = RymParser::new(tokens).parse_file()
        .map_err(|e| miette::miette!("parse error: {e}"))?;

    // Resolve imports: merge each imported file's def_zone into this file.
    let base_dir = cli.input.parent().unwrap_or(Path::new("."));
    resolve_imports(&mut ast, base_dir)?;

    if cli.dump_ast {
        println!("{:#?}", ast);
        return Ok(());
    }

    // ── Phase 3: Semantic analysis ───────────────────────────
    let errors = TyChecker::new().check(&ast);
    if !errors.is_empty() {
        for e in &errors { eprintln!("error: {e}"); }
        return Err(miette::miette!("{} semantic error(s)", errors.len()));
    }

    // ── Phase 4: IR lowering ─────────────────────────────────
    let ir = Lowerer::new().lower_file(&ast, &module_name);

    if cli.dump_ir {
        println!("{}", dump_ir(&ir));
        return Ok(());
    }

    let base_out: PathBuf = cli.output.clone().unwrap_or_else(|| {
        cli.input.with_extension("")
    });

    match cli.target.as_str() {
        "c" | "C" => compile_c(&cli, &ir, &base_out),
        "la64" | "loongarch64" => compile_la64(&cli, &ir, &base_out),
        other => Err(miette::miette!("unknown target '{other}' — use 'c' or 'la64'")),
    }
}

// ── Import resolution ─────────────────────────────────────────

/// Recursively resolve `import "path"` items, merging their def_zones.
/// Cycles are detected by tracking visited canonical paths.
fn resolve_imports(file: &mut SourceFile, base_dir: &Path) -> Result<()> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    resolve_imports_inner(file, base_dir, &mut visited)
}

fn resolve_imports_inner(
    file: &mut SourceFile,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<()> {
    let mut i = 0;
    while i < file.def_zone.len() {
        if let ItemKind::Import(path_str) = &file.def_zone[i].kind {
            let path_str = path_str.clone();
            let import_path = base_dir.join(&path_str);
            let canonical = import_path.canonicalize()
                .unwrap_or_else(|_| import_path.clone());

            if visited.contains(&canonical) {
                // Already merged — remove the import stub and continue.
                file.def_zone.remove(i);
                continue;
            }
            visited.insert(canonical.clone());

            let src = std::fs::read_to_string(&import_path)
                .map_err(|e| miette::miette!("import '{}': {e}", import_path.display()))?;
            let tokens = Lexer::new(&src).tokenize()
                .map_err(|e| miette::miette!("import '{}' lex error: {e}", path_str))?;
            let mut imported = RymParser::new(tokens).parse_file()
                .map_err(|e| miette::miette!("import '{}' parse error: {e}", path_str))?;

            let import_dir = import_path.parent().unwrap_or(Path::new("."));
            resolve_imports_inner(&mut imported, import_dir, visited)?;

            // Remove the Import stub, splice in the imported def_zone at the same position.
            file.def_zone.remove(i);
            let items = imported.def_zone;
            let n = items.len();
            for (j, item) in items.into_iter().enumerate() {
                file.def_zone.insert(i + j, item);
            }
            i += n;
        } else {
            i += 1;
        }
    }
    Ok(())
}

// ── C backend ────────────────────────────────────────────────

fn compile_c(
    cli: &Cli,
    ir: &rym_ir::IrModule,
    base_out: &Path,
) -> Result<()> {
    let c_path = base_out.with_extension("c");

    // Generate C source.
    let mut gen = CCodegen::new();
    let mut c_src = gen.emit_module(ir);

    // Inject I/O helpers when any of print/println/puts are used.
    let needs_io = c_src.contains("__rym_print")
        || c_src.contains("__rym_println")
        || c_src.contains("__rym_puts");
    if needs_io {
        let helper = rym_codegen::c::io_helpers();
        if let Some(pos) = c_src.find('\n') {
            c_src.insert_str(pos + 1, helper);
        }
    }

    std::fs::write(&c_path, &c_src).into_diagnostic()?;
    eprintln!("rymc: wrote {}", c_path.display());

    if cli.emit_only {
        return Ok(());
    }

    // Compile with system C compiler.
    let exe = base_out.to_path_buf();
    run_cmd(&cli.cc, &[
        c_path.to_str().unwrap(),
        "-O2",
        "-o", exe.to_str().unwrap(),
    ])?;

    // Remove intermediate .c file.
    let _ = std::fs::remove_file(&c_path);

    eprintln!("rymc: built {}", exe.display());
    Ok(())
}

// ── LA64 backend ─────────────────────────────────────────────

fn compile_la64(
    cli: &Cli,
    ir: &rym_ir::IrModule,
    base_out: &Path,
) -> Result<()> {
    let asm_path = base_out.with_extension("s");
    let obj_path = base_out.with_extension("o");
    let rt_obj   = base_out.with_extension("rt.o");
    let exe      = base_out.to_path_buf();

    let asm = Codegen::new().emit_module(ir);
    std::fs::write(&asm_path, &asm).into_diagnostic()?;
    eprintln!("rymc: wrote assembly {}", asm_path.display());

    if cli.emit_only {
        return Ok(());
    }

    let runtime_s = find_runtime(&cli.runtime)?;

    run_cmd(&cli.assembler, &["-mla64v1.0", asm_path.to_str().unwrap(), "-o", obj_path.to_str().unwrap()])?;
    run_cmd(&cli.assembler, &["-mla64v1.0", runtime_s.to_str().unwrap(), "-o", rt_obj.to_str().unwrap()])?;
    run_cmd(&cli.linker,    &["-static", rt_obj.to_str().unwrap(), obj_path.to_str().unwrap(), "-o", exe.to_str().unwrap()])?;

    let _ = std::fs::remove_file(&obj_path);
    let _ = std::fs::remove_file(&rt_obj);
    let _ = std::fs::remove_file(&asm_path);

    eprintln!("rymc: built {}", exe.display());
    Ok(())
}

// ── Utilities ─────────────────────────────────────────────────

fn find_runtime(override_path: &Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        if p.exists() { return Ok(p.clone()); }
        return Err(miette::miette!("runtime not found at {}", p.display()));
    }
    let candidates = ["runtime/start.s", "../runtime/start.s", "../../runtime/start.s"];
    if let Ok(exe) = std::env::current_exe() {
        for rel in &candidates {
            let p = exe.parent().unwrap_or(Path::new(".")).join(rel);
            if p.exists() { return Ok(p); }
        }
    }
    for rel in &candidates {
        let p = PathBuf::from(rel);
        if p.exists() { return Ok(p); }
    }
    Err(miette::miette!("cannot find runtime/start.s — use --runtime <path>"))
}

fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| miette::miette!("failed to run '{program}': {e}"))?;
    if !status.success() {
        return Err(miette::miette!("'{program}' exited with {status}"));
    }
    Ok(())
}
