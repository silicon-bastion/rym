use clap::Parser;
use miette::{IntoDiagnostic, Result};
use rym_lexer::Lexer;
use rym_parser::Parser as RymParser;
use rym_sema::TyChecker;
use rym_ir::lower::Lowerer;
use rym_codegen::{Codegen, la64::dump_ir};
use std::path::{Path, PathBuf};
use std::process::Command;

/// rymc — the Rym language bootstrap compiler
#[derive(Parser)]
#[command(name = "rymc", version, about = "Rym language compiler targeting LoongArch64")]
struct Cli {
    /// Source file to compile
    input: PathBuf,

    /// Output executable (default: same name as input without extension)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Only emit assembly, do not link  (saves to <output>.s)
    #[arg(long)]
    emit_asm: bool,

    /// Dump token stream and exit
    #[arg(long)]
    dump_tokens: bool,

    /// Dump AST and exit
    #[arg(long)]
    dump_ast: bool,

    /// Dump IR and exit
    #[arg(long)]
    dump_ir: bool,

    /// Path to the Rym runtime start.s (auto-detected if not given)
    #[arg(long)]
    runtime: Option<PathBuf>,

    /// Assembler binary to use (default: as)
    #[arg(long, default_value = "as")]
    assembler: String,

    /// Linker binary to use (default: ld)
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
        for tok in &tokens {
            println!("{:?}", tok);
        }
        return Ok(());
    }

    // ── Phase 2: Parse ───────────────────────────────────────
    let ast = RymParser::new(tokens).parse_file()
        .map_err(|e| miette::miette!("parse error: {e}"))?;

    if cli.dump_ast {
        println!("{:#?}", ast);
        return Ok(());
    }

    // ── Phase 3: Semantic analysis ───────────────────────────
    let errors = TyChecker::new().check(&ast);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        return Err(miette::miette!("{} semantic error(s)", errors.len()));
    }

    // ── Phase 4: IR lowering ─────────────────────────────────
    let ir = Lowerer::new().lower_file(&ast, &module_name);

    if cli.dump_ir {
        println!("{}", dump_ir(&ir));
        return Ok(());
    }

    // ── Phase 5/6: Codegen → LoongArch64 assembly ────────────
    let asm = Codegen::new().emit_module(&ir);

    // Determine output paths.
    let base_out: PathBuf = cli.output.clone().unwrap_or_else(|| {
        cli.input.with_extension("")
    });
    let asm_path = base_out.with_extension("s");

    std::fs::write(&asm_path, &asm).into_diagnostic()?;
    eprintln!("rymc: wrote assembly {}", asm_path.display());

    if cli.emit_asm {
        return Ok(());
    }

    // ── Assemble + Link ──────────────────────────────────────
    let obj_path  = base_out.with_extension("o");
    let rt_obj    = base_out.with_extension("rt.o");
    let exe_path  = base_out.clone();

    // Find runtime start.s — look next to the compiler binary first,
    // then at a hard-coded path relative to the repo.
    let runtime_s = find_runtime(&cli.runtime)?;

    // Assemble user code.
    run_cmd(&cli.assembler, &[
        "-mla64v1.0",
        asm_path.to_str().unwrap(),
        "-o", obj_path.to_str().unwrap(),
    ])?;

    // Assemble runtime.
    run_cmd(&cli.assembler, &[
        "-mla64v1.0",
        runtime_s.to_str().unwrap(),
        "-o", rt_obj.to_str().unwrap(),
    ])?;

    // Link: runtime first so _start comes first.
    run_cmd(&cli.linker, &[
        "-static",
        rt_obj.to_str().unwrap(),
        obj_path.to_str().unwrap(),
        "-o", exe_path.to_str().unwrap(),
    ])?;

    // Clean up intermediates.
    let _ = std::fs::remove_file(&obj_path);
    let _ = std::fs::remove_file(&rt_obj);
    let _ = std::fs::remove_file(&asm_path);

    eprintln!("rymc: built {}", exe_path.display());
    Ok(())
}

fn find_runtime(override_path: &Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        if p.exists() {
            return Ok(p.clone());
        }
        return Err(miette::miette!("runtime not found at {}", p.display()));
    }

    // Search order:
    // 1. Next to the rymc binary.
    // 2. <repo_root>/runtime/start.s  (development layout).
    let candidates: &[&str] = &[
        "runtime/start.s",
        "../runtime/start.s",
        "../../runtime/start.s",
    ];

    // Relative to current exe.
    if let Ok(exe) = std::env::current_exe() {
        for rel in candidates {
            let p = exe.parent().unwrap_or(Path::new(".")).join(rel);
            if p.exists() {
                return Ok(p);
            }
        }
    }

    // Relative to current working directory.
    for rel in candidates {
        let p = PathBuf::from(rel);
        if p.exists() {
            return Ok(p);
        }
    }

    Err(miette::miette!(
        "cannot find runtime/start.s — use --runtime <path> to specify it"
    ))
}

fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| miette::miette!("failed to run {program}: {e}"))?;

    if !status.success() {
        return Err(miette::miette!(
            "{program} exited with status {}", status
        ));
    }
    Ok(())
}
