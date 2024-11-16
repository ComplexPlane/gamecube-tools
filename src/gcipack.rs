use std::fmt::Display;

use thiserror::Error;

#[derive(Debug)]
pub enum StringKind {
    FileName,
    Title,
    Description,
    GameCode,
}

impl Display for StringKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StringKind::FileName => "file name",
            StringKind::Title => "title",
            StringKind::Description => "description",
            StringKind::GameCode => "game code",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug)]
pub enum ImageKind {
    Banner,
    Icon,
}

impl Display for ImageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ImageKind::Banner => "banner",
            ImageKind::Icon => "icon",
        };
        write!(f, "{}", s)
    }
}

#[derive(Error, Debug)]
pub enum GciPackError {
    #[error("invalid {kind} image size: {info}")]
    ImageInvalidSize { kind: ImageKind, info: String },
    #[error("invalid {kind} size: {info}")]
    StringInvalidSize { kind: StringKind, info: String },
    #[error("{0} is non-ASCII")]
    StringNonAscii(StringKind),
    #[error("I/O error")]
    IoError(#[from] std::io::Error),
}

const MAX_FILE_NAME_SIZE: usize = 0x20;
const MAX_TITLE_SIZE: usize = 0x20;
const MAX_DESCRIPTION_SIZE: usize = 0x20;
const GAME_CODE_SIZE: usize = 6;

const BANNER_SIZE: usize = 0x1800;
const ICON_SIZE: usize = 0x800;

fn validate_str(s: &str, kind: StringKind, max_size: usize) -> Result<(), GciPackError> {
    if !s.is_ascii() {
        return Err(GciPackError::StringNonAscii(kind));
    }
    if s.len() > max_size {
        return Err(GciPackError::StringInvalidSize {
            kind,
            info: format!("max size is {}, found {}", max_size, s.len()),
        });
    }
    Ok(())
}

fn validate(
    file: &[u8],
    file_name: &str,
    title: &str,
    description: &str,
    banner: &[u8],
    icon: &[u8],
    gamecode: &str,
) -> Result<(), GciPackError> {
    validate_str(file_name, StringKind::FileName, MAX_FILE_NAME_SIZE)?;
    validate_str(title, StringKind::Title, MAX_TITLE_SIZE)?;
    validate_str(description, StringKind::Description, MAX_DESCRIPTION_SIZE)?;

    if !gamecode.is_ascii() {
        return Err(GciPackError::StringNonAscii(StringKind::GameCode));
    }
    if gamecode.chars().count() != GAME_CODE_SIZE {
        return Err(GciPackError::StringInvalidSize {
            kind: StringKind::GameCode,
            info: format!(
                "expected {}, found {}",
                GAME_CODE_SIZE,
                gamecode.chars().count()
            ),
        });
    }

    if banner.len() != BANNER_SIZE {
        return Err(GciPackError::ImageInvalidSize {
            kind: ImageKind::Banner,
            info: format!("should be {} (96x32 RGB5A3)", BANNER_SIZE),
        });
    }
    if icon.len() != ICON_SIZE {
        return Err(GciPackError::ImageInvalidSize {
            kind: ImageKind::Icon,
            info: format!("should be {} (32x32 RGB5A3)", ICON_SIZE),
        });
    }

    Ok(())
}

fn generate_gci(
    file: &[u8],
    file_name: &str,
    title: &str,
    description: &str,
    banner: &[u8],
    icon: &[u8],
    gamecode: &str,
) -> Vec<u8> {
    Vec::new()
}

pub fn gcipack(
    file: &[u8],
    file_name: &str,
    title: &str,
    description: &str,
    banner: &[u8],
    icon: &[u8],
    gamecode: &str,
) -> Result<Vec<u8>, GciPackError> {
    validate(file, file_name, title, description, banner, icon, gamecode)?;
    Ok(generate_gci(
        file,
        file_name,
        title,
        description,
        banner,
        icon,
        gamecode,
    ))
}
