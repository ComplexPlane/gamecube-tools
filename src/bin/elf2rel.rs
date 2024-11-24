use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use anyhow::Context;
use gamecube_tools::elf2rel::{self, RelVersion};

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Elf2RelArgs {
    input_elf: PathBuf,
    input_symbol_map: PathBuf,
    #[arg(short, long)]
    output_rel: Option<PathBuf>,
    #[arg(long, default_value_t = 0x1000)]
    rel_id: u32,
    #[arg(long, default_value_t = 3)]
    rel_version: u8,
}

fn read_file<P>(p: P) -> anyhow::Result<Vec<u8>>
where
    P: AsRef<Path>,
{
    std::fs::read(&p).with_context(|| format!("cannot read {}", p.as_ref().to_string_lossy()))
}

fn main() -> anyhow::Result<()> {
    let args = Elf2RelArgs::parse();
    let input_elf = read_file(&args.input_elf)?;
    let input_symbol_map = read_file(&args.input_symbol_map)?;
    let output_rel_path = args
        .output_rel
        .unwrap_or(args.input_elf.with_extension("rel"));
    let rel_version = RelVersion::try_from(args.rel_version)
        .map_err(|_| anyhow!("Invalid REL version: {}", args.rel_version))?;

    let rel = elf2rel::elf2rel(&input_elf, &input_symbol_map, args.rel_id, rel_version)?;

    let mut output_file = File::create(output_rel_path)?;
    output_file.write_all(&rel)?;

    Ok(())
}
