use clap::Parser;
use miette::{IntoDiagnostic, Result};
use rym_lexer::Lexer;
use std::path::PathBuf;

/// rymc — the Rym language bootstrap compiler
#[derive(Parser)]
#[command(name = "rymc", version, about)]
struct Cli {
    /// Source file to compile
    input: PathBuf,

    /// Output file (defaults to `./out`)
    #[arg(short, long, default_value = "out")]
    output: PathBuf,

    /// Dump the token stream and exit
    #[arg(long)]
    dump_tokens: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let src = std::fs::read_to_string(&cli.input).into_diagnostic()?;

    let tokens = Lexer::new(&src).tokenize().map_err(|e| {
        miette::miette!("{e}")
    })?;

    if cli.dump_tokens {
        for tok in &tokens {
            println!("{:?}", tok);
        }
        return Ok(());
    }

    // TODO: parser → sema → ir → codegen
    println!("rymc: lexed {} tokens from {:?}", tokens.len(), cli.input);

    Ok(())
}
