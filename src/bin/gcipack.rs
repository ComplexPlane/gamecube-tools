use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Context;
use gamecube_tools::gcipack;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct GciPackArgs {
    input: PathBuf,
    file_name: String,
    title: String,
    description: String,
    banner: PathBuf,
    icon: PathBuf,
    gamecode: String,
}

fn read_file<P>(p: P) -> anyhow::Result<Vec<u8>>
where
    P: AsRef<Path>,
{
    std::fs::read(&p).with_context(|| format!("cannot read {}", p.as_ref().to_string_lossy()))
}

fn main() -> anyhow::Result<()> {
    let args = GciPackArgs::parse();
    let input = read_file(&args.input)?;
    let banner = read_file(&args.banner)?;
    let icon = read_file(&args.icon)?;
    let gci = gcipack::gcipack(
        &input,
        &args.file_name,
        &args.title,
        &args.description,
        &banner,
        &icon,
        &args.gamecode,
    )?;
    let mut output_file = File::create(args.input.with_extension("gci"))?;
    output_file.write_all(&gci)?;

    Ok(())
}
