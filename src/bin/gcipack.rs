use std::{fs::File, io::Write, path::PathBuf};

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

fn main() -> anyhow::Result<()> {
    let args = GciPackArgs::parse();
    let input = std::fs::read(&args.input)?;
    let banner = std::fs::read(&args.banner)?;
    let icon = std::fs::read(&args.icon)?;
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
