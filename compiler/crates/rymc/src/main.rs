use clap::Parser;
use miette::{IntoDiagnostic, Result};
use rym_lexer::Lexer;
use rym_parser::Parser as RymParser;
use rym_sema::TyChecker;
use rym_ir::lower::Lowerer;
use rym_codegen::{Codegen, la64::dump_ir};
use std::path::PathBuf;

/// rymc — the Rym language bootstrap compiler
#[derive(Parser)]
#[command(name = "rymc", version, about)]
struct Cli {
    /// Source file to compile
    input: PathBuf,

    /// Output file (defaults to `./out.s`)
    #[arg(short, long, default_value = "out.s")]
    output: PathBuf,

    /// Dump the token stream and exit
    #[arg(long)]
    dump_tokens: bool,

    /// Dump the AST and exit
    #[arg(long)]
    dump_ast: bool,

    /// Dump the IR and exit
    #[arg(long)]
    dump_ir: bool,
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

    std::fs::write(&cli.output, &asm).into_diagnostic()?;
    eprintln!("rymc: wrote {} ({} bytes)", cli.output.display(), asm.len());

    Ok(())
}
