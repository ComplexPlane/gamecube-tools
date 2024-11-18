use std::{fmt::Display, iter, time::SystemTime};

use thiserror::Error;
use zerocopy::byteorder::big_endian;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

const MAX_FILE_NAME_SIZE: usize = 0x20;
const MAX_TITLE_SIZE: usize = 0x20;
const MAX_DESCRIPTION_SIZE: usize = 0x20;

const BANNER_SIZE: usize = 0x1800;
const ICON_SIZE: usize = 0x800;
const FILE_HEADER_SIZE: usize = 0x200;
const GAME_CODE_SIZE: usize = 6;
const BLOCK_SIZE: usize = 0x2000;
const FILE_HEADER_PADDING_SIZE: usize =
    FILE_HEADER_SIZE - MAX_TITLE_SIZE - MAX_DESCRIPTION_SIZE - size_of::<u32>();

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
}

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

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct GciHeader {
    gamecode: [u8; 6],
    unused0: u8,
    banner_fmt: u8,
    filename: [u8; MAX_FILE_NAME_SIZE],
    last_modified: big_endian::U32,
    image_offset: big_endian::U32,
    icon_format: big_endian::U16,
    icon_speed: big_endian::U16,
    permissions: u8,
    copy_times: u8,
    first_block_num: big_endian::U16,
    block_count: big_endian::U16,
    unused1: big_endian::U16,
    comment_offset: big_endian::U32,
}

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct GciFileMetadata {
    banner: [u8; BANNER_SIZE],
    icon: [u8; ICON_SIZE],
    title: [u8; MAX_TITLE_SIZE],
    description: [u8; MAX_DESCRIPTION_SIZE],
    file_size: big_endian::U32,
    padding: [u8; FILE_HEADER_PADDING_SIZE],
}

fn get_modified_time_sec() -> u32 {
    let base = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(946684800); // Jan 1 2000
    let now = SystemTime::now();
    now.duration_since(base).unwrap().as_secs() as u32
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
    let unpadded_gci_file_size = size_of::<GciFileMetadata>() + file.len();
    let blocks = unpadded_gci_file_size.div_ceil(BLOCK_SIZE);
    let gci_file_size = blocks * BLOCK_SIZE;

    let mut gci = Vec::with_capacity(size_of::<GciHeader>() + gci_file_size);

    // Build header
    let header = GciHeader {
        gamecode: str_to_array(gamecode, StringKind::GameCode)?,
        unused0: 0xff,
        banner_fmt: 2,
        filename: str_to_padded_array(file_name, StringKind::FileName)?,
        last_modified: get_modified_time_sec().into(),
        image_offset: 0.into(),
        icon_format: 2.into(),
        icon_speed: 3.into(),
        permissions: 4,
        copy_times: 0,
        first_block_num: 0.into(),
        block_count: (blocks as u16).into(),
        unused1: 0xff.into(),
        comment_offset: ((BANNER_SIZE + ICON_SIZE) as u32).into(),
    };

    // Build file metadata
    let banner = banner
        .try_into()
        .map_err(|_| GciPackError::ImageInvalidSize {
            kind: ImageKind::Banner,
            info: format!("should be {} (96x32 RGB5A3)", BANNER_SIZE),
        })?;
    let icon = icon
        .try_into()
        .map_err(|_| GciPackError::ImageInvalidSize {
            kind: ImageKind::Icon,
            info: format!("should be {} (32x32 RGB5A3)", ICON_SIZE),
        })?;
    let metadata = GciFileMetadata {
        banner,
        icon,
        title: str_to_padded_array(title, StringKind::Title)?,
        description: str_to_padded_array(description, StringKind::Description)?,
        file_size: (file.len() as u32).into(),
        padding: [0; FILE_HEADER_PADDING_SIZE],
    };

    // Combine everything
    gci.extend_from_slice(header.as_bytes());
    gci.extend_from_slice(metadata.as_bytes());
    gci.extend_from_slice(file);
    gci.extend_from_slice(&vec![0; gci_file_size - unpadded_gci_file_size]);

    Ok(gci)
}

fn str_to_array<const N: usize>(input: &str, kind: StringKind) -> Result<[u8; N], GciPackError> {
    if !input.is_ascii() {
        return Err(GciPackError::StringNonAscii(kind));
    }

    input
        .as_bytes()
        .try_into()
        .map_err(|_| GciPackError::StringInvalidSize {
            kind,
            info: format!("expected {}, got {}", N, input.len()),
        })
}

fn str_to_padded_array<const N: usize>(
    input: &str,
    kind: StringKind,
) -> Result<[u8; N], GciPackError> {
    if !input.is_ascii() {
        return Err(GciPackError::StringNonAscii(kind));
    }

    if input.len() > N {
        return Err(GciPackError::StringInvalidSize {
            kind,
            info: format!("max size is {}, got {}", N, input.len()),
        });
    }

    let mut array = [0; N];
    array[..input.len()].copy_from_slice(input.as_bytes());
    Ok(array)
}
