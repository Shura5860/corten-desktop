//! Color management, ICC profile parsing, gamut mapping, and DRM HDR metadata handling.
//!
//! Provides color space math (sRGB, Display P3, BT.2020), EOTF transfer curves
//! (sRGB gamma and SMPTE ST 2084 PQ), Linux DRM `hdr_output_metadata` serialization,
//! and standard ICC profile inspection.

use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ColorManagementError {
    #[error("ICC profile too small: {0} bytes, expected at least 128 bytes")]
    ProfileTooSmall(usize),
    #[error("Invalid ICC magic signature: {0:?}, expected b\"acsp\"")]
    InvalidSignature([u8; 4]),
    #[error("Corrupted ICC tag table or tag offset: {0}")]
    CorruptTagTable(String),
    #[error("File I/O error reading ICC profile: {0}")]
    IoError(String),
}

/// Supported display color spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorSpace {
    #[default]
    Srgb,
    DciP3,
    Bt2020,
}

impl ColorSpace {
    pub fn parse_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "dci-p3" | "dcip3" | "p3" | "display-p3" => Self::DciP3,
            "bt2020" | "bt.2020" | "rec2020" | "rec.2020" | "hdr" => Self::Bt2020,
            _ => Self::Srgb,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::DciP3 => "dci-p3",
            Self::Bt2020 => "bt2020",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Srgb => "sRGB (Standart)",
            Self::DciP3 => "DCI-P3 (Geniş Gamut)",
            Self::Bt2020 => "BT.2020 (HDR / Geniş Renk)",
        }
    }
}

/// CIE 1931 chromaticity coordinates (x, y) for Red, Green, Blue, and White Point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorPrimaries {
    pub red: [f32; 2],
    pub green: [f32; 2],
    pub blue: [f32; 2],
    pub white: [f32; 2],
}

impl ColorPrimaries {
    pub fn srgb() -> Self {
        Self {
            red: [0.640, 0.330],
            green: [0.300, 0.600],
            blue: [0.150, 0.060],
            white: [0.3127, 0.3290], // D65
        }
    }

    pub fn dci_p3() -> Self {
        Self {
            red: [0.680, 0.320],
            green: [0.265, 0.690],
            blue: [0.150, 0.060],
            white: [0.3127, 0.3290], // Display P3 uses D65
        }
    }

    pub fn bt2020() -> Self {
        Self {
            red: [0.708, 0.292],
            green: [0.170, 0.797],
            blue: [0.131, 0.046],
            white: [0.3127, 0.3290], // D65
        }
    }

    /// Convert chromaticity coordinate (0.0 .. 1.0) to DRM 0.00002 fixed-point integer.
    pub fn to_drm_fixed(coord: f32) -> u16 {
        let val = (coord / 0.00002).round();
        val.clamp(0.0, 65535.0) as u16
    }
}

// ---------------------------------------------------------------------------
// Gamut Mapping (3x3 Matrix Transformations in Linear RGB)
// ---------------------------------------------------------------------------

/// 3x3 Linear RGB gamut transformation matrix.
pub type Mat3 = [[f32; 3]; 3];

/// Returns the 3x3 color conversion matrix from source gamut to target gamut.
pub fn gamut_matrix(from: ColorSpace, to: ColorSpace) -> Mat3 {
    match (from, to) {
        (ColorSpace::Srgb, ColorSpace::Srgb)
        | (ColorSpace::DciP3, ColorSpace::DciP3)
        | (ColorSpace::Bt2020, ColorSpace::Bt2020) => [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ],
        // sRGB -> DCI-P3 (Display P3)
        (ColorSpace::Srgb, ColorSpace::DciP3) => [
            [0.82246, 0.17754, 0.00000],
            [0.03319, 0.96681, 0.00000],
            [0.01708, 0.07240, 0.91052],
        ],
        // DCI-P3 -> sRGB
        (ColorSpace::DciP3, ColorSpace::Srgb) => [
            [1.22494, -0.22494, 0.00000],
            [-0.04206, 1.04206, 0.00000],
            [-0.01965, -0.07864, 1.09829],
        ],
        // sRGB -> BT.2020
        (ColorSpace::Srgb, ColorSpace::Bt2020) => [
            [0.62740, 0.32928, 0.04332],
            [0.06909, 0.91954, 0.01136],
            [0.01639, 0.08801, 0.89560],
        ],
        // BT.2020 -> sRGB
        (ColorSpace::Bt2020, ColorSpace::Srgb) => [
            [1.66050, -0.58764, -0.07286],
            [-0.12455, 1.13290, -0.00835],
            [-0.01815, -0.10058, 1.11873],
        ],
        // DCI-P3 -> BT.2020
        (ColorSpace::DciP3, ColorSpace::Bt2020) => [
            [0.75383, 0.19860, 0.04757],
            [0.04574, 0.94178, 0.01248],
            [-0.00121, 0.01760, 0.98361],
        ],
        // BT.2020 -> DCI-P3
        (ColorSpace::Bt2020, ColorSpace::DciP3) => [
            [1.34358, -0.28218, -0.06140],
            [-0.06530, 1.07579, -0.01049],
            [0.00282, -0.01960, 1.01678],
        ],
    }
}

/// Applies a 3x3 matrix to an [R, G, B] triple.
pub fn transform_color(rgb: [f32; 3], mat: &Mat3) -> [f32; 3] {
    [
        (mat[0][0] * rgb[0] + mat[0][1] * rgb[1] + mat[0][2] * rgb[2]).clamp(0.0, 1.0),
        (mat[1][0] * rgb[0] + mat[1][1] * rgb[1] + mat[1][2] * rgb[2]).clamp(0.0, 1.0),
        (mat[2][0] * rgb[0] + mat[2][1] * rgb[1] + mat[2][2] * rgb[2]).clamp(0.0, 1.0),
    ]
}

// ---------------------------------------------------------------------------
// EOTF Transfer Curves (Gamma & SMPTE ST 2084 PQ)
// ---------------------------------------------------------------------------

/// Convert non-linear sRGB channel to linear light.
pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert linear light channel to non-linear sRGB.
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// SMPTE ST 2084 (Perceptual Quantizer / PQ) constants
const PQ_M1: f32 = 2610.0 / 16384.0; // ~0.1593017578
const PQ_M2: f32 = (2523.0 / 4096.0) * 128.0; // ~78.84375
const PQ_C1: f32 = 3424.0 / 4096.0; // ~0.8359375
const PQ_C2: f32 = (2413.0 / 4096.0) * 32.0; // ~18.8515625
const PQ_C3: f32 = (2392.0 / 4096.0) * 32.0; // ~18.6875

/// Converts absolute luminance in cd/m² (nits) to SMPTE ST 2084 PQ signal (0.0 .. 1.0).
/// Reference peak luminance is 10,000 cd/m².
pub fn linear_to_pq(luminance_nits: f32) -> f32 {
    let y = (luminance_nits / 10000.0).clamp(0.0, 1.0);
    let y_m1 = y.powf(PQ_M1);
    let num = PQ_C1 + PQ_C2 * y_m1;
    let den = 1.0 + PQ_C3 * y_m1;
    (num / den).powf(PQ_M2).clamp(0.0, 1.0)
}

/// Converts SMPTE ST 2084 PQ signal (0.0 .. 1.0) to absolute luminance in cd/m² (nits).
pub fn pq_to_linear(pq: f32) -> f32 {
    let n = pq.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
    let num = (n - PQ_C1).max(0.0);
    let den = PQ_C2 - PQ_C3 * n;
    if den <= 0.0 {
        return 10000.0;
    }
    let y = (num / den).powf(1.0 / PQ_M1);
    (y * 10000.0).clamp(0.0, 10000.0)
}

// ---------------------------------------------------------------------------
// Linux DRM `hdr_output_metadata` C Struct Serialization
// ---------------------------------------------------------------------------

/// Linux DRM kernel `hdr_metadata_infoframe` (matches `<drm/drm_mode.h>`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmHdrMetadataInfoframe {
    /// 0 = SDR gamma, 1 = HDR traditional gamma, 2 = SMPTE ST2084 PQ, 3 = HLG
    pub eotf: u8,
    /// 0 = Static Metadata Type 1
    pub metadata_type: u8,
    /// Color primaries [R, G, B] (x, y) in units of 0.00002
    pub display_primaries: [[u16; 2]; 3],
    /// White point (x, y) in units of 0.00002
    pub white_point: [u16; 2],
    /// Max display mastering luminance in 1 cd/m²
    pub max_display_mastering_luminance: u16,
    /// Min display mastering luminance in 0.0001 cd/m²
    pub min_display_mastering_luminance: u16,
    /// Max Content Light Level in 1 cd/m²
    pub max_cll: u16,
    /// Max Frame Average Light Level in 1 cd/m²
    pub max_fall: u16,
}

/// Linux DRM kernel `hdr_output_metadata` structure (matches `<drm/drm_mode.h>`).
/// Total size is 32 bytes (4-byte aligned).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmHdrOutputMetadata {
    pub metadata_type: u32,
    pub hdmi_metadata_type1: DrmHdrMetadataInfoframe,
    pub _reserved: [u8; 2],
}

impl DrmHdrOutputMetadata {
    /// Builds HDR10 metadata (SMPTE ST2084 PQ) for a given color space and peak luminance.
    pub fn hdr10(color_space: ColorSpace, max_luminance_nits: u16) -> Self {
        let primaries = match color_space {
            ColorSpace::Srgb => ColorPrimaries::srgb(),
            ColorSpace::DciP3 => ColorPrimaries::dci_p3(),
            ColorSpace::Bt2020 => ColorPrimaries::bt2020(),
        };

        let dp = [
            [
                ColorPrimaries::to_drm_fixed(primaries.red[0]),
                ColorPrimaries::to_drm_fixed(primaries.red[1]),
            ],
            [
                ColorPrimaries::to_drm_fixed(primaries.green[0]),
                ColorPrimaries::to_drm_fixed(primaries.green[1]),
            ],
            [
                ColorPrimaries::to_drm_fixed(primaries.blue[0]),
                ColorPrimaries::to_drm_fixed(primaries.blue[1]),
            ],
        ];

        let wp = [
            ColorPrimaries::to_drm_fixed(primaries.white[0]),
            ColorPrimaries::to_drm_fixed(primaries.white[1]),
        ];

        let max_nits = max_luminance_nits.clamp(300, 10000);
        let max_fall = ((max_nits as u32 * 4) / 5) as u16;

        Self {
            metadata_type: 0, // DRM Static Metadata Type 1
            hdmi_metadata_type1: DrmHdrMetadataInfoframe {
                eotf: 2, // SMPTE ST 2084 PQ
                metadata_type: 0,
                display_primaries: dp,
                white_point: wp,
                max_display_mastering_luminance: max_nits,
                min_display_mastering_luminance: 1, // 0.0001 cd/m^2 (OLED/High contrast)
                max_cll: max_nits,
                max_fall,
            },
            _reserved: [0; 2],
        }
    }

    /// Convert struct to raw bytes for `drmModeCreatePropertyBlob` / `create_property_blob`.
    pub fn to_raw_bytes(&self) -> Vec<u8> {
        let size = std::mem::size_of::<Self>();
        let ptr = self as *const Self as *const u8;
        unsafe { std::slice::from_raw_parts(ptr, size).to_vec() }
    }
}

// ---------------------------------------------------------------------------
// Standard ICC Profile Inspector
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IccProfileInfo {
    pub description: String,
    pub color_space: String,
    pub device_class: String,
    pub version_major: u8,
    pub version_minor: u8,
    pub size: usize,
}

/// Parses an ICC profile header and extracts description, device class, and color space.
pub fn parse_icc_profile(data: &[u8]) -> Result<IccProfileInfo, ColorManagementError> {
    if data.len() < 128 {
        return Err(ColorManagementError::ProfileTooSmall(data.len()));
    }

    // Check magic signature at offset 36..40 (b"acsp")
    let magic = [data[36], data[37], data[38], data[39]];
    if &magic != b"acsp" {
        return Err(ColorManagementError::InvalidSignature(magic));
    }

    let declared_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let version_major = data[8];
    let version_minor = data[9] >> 4;

    let device_class = match &data[12..16] {
        b"mntr" => "Monitör (Ekran)".to_string(),
        b"prtr" => "Yazıcı".to_string(),
        b"scnr" => "Tarayıcı".to_string(),
        b"link" => "Aygıt Bağlantısı".to_string(),
        b"spac" => "Renk Uzayı Dönüşümü".to_string(),
        b"abst" => "Soyut Profil".to_string(),
        _ => String::from_utf8_lossy(&data[12..16]).trim().to_string(),
    };

    let color_space = match &data[16..20] {
        b"RGB " => "RGB".to_string(),
        b"CMYK" => "CMYK".to_string(),
        b"GRAY" => "Gri Tonlama".to_string(),
        b"XYZ " => "CIE XYZ".to_string(),
        b"Lab " => "CIE L*a*b*".to_string(),
        _ => String::from_utf8_lossy(&data[16..20]).trim().to_string(),
    };

    let mut description = format!("ICC Profil v{}.{}", version_major, version_minor);

    // Search tag table (starts at byte 128)
    if data.len() >= 132 {
        let tag_count = u32::from_be_bytes([data[128], data[129], data[130], data[131]]) as usize;
        let mut offset = 132;

        for _ in 0..tag_count {
            if offset + 12 > data.len() {
                break;
            }
            let tag_sig = &data[offset..offset + 4];
            let tag_offset = u32::from_be_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]) as usize;
            let tag_size = u32::from_be_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]) as usize;

            if tag_sig == b"desc" && tag_offset + tag_size <= data.len() && tag_size >= 12 {
                let tag_data = &data[tag_offset..tag_offset + tag_size];
                let type_sig = &tag_data[0..4];

                if type_sig == b"desc" && tag_size >= 12 {
                    // textDescriptionType: 4 bytes type, 4 bytes reserved, 4 bytes ASCII count
                    let ascii_len = u32::from_be_bytes([
                        tag_data[8],
                        tag_data[9],
                        tag_data[10],
                        tag_data[11],
                    ]) as usize;
                    if ascii_len > 1 && 12 + ascii_len <= tag_size {
                        let text_slice = &tag_data[12..12 + ascii_len - 1]; // omit trailing null
                        if let Ok(desc) = std::str::from_utf8(text_slice) {
                            description = desc.trim().to_string();
                            break;
                        }
                    }
                } else if type_sig == b"mluc" && tag_size >= 28 {
                    // multiLocalizedUnicodeType: find first unicode record
                    let rec_offset = u32::from_be_bytes([
                        tag_data[20],
                        tag_data[21],
                        tag_data[22],
                        tag_data[23],
                    ]) as usize;
                    let rec_len = u32::from_be_bytes([
                        tag_data[16],
                        tag_data[17],
                        tag_data[18],
                        tag_data[19],
                    ]) as usize;
                    if rec_offset + rec_len <= tag_size {
                        #[allow(clippy::chunks_exact_to_as_chunks)]
                        let u16_data: Vec<u16> = tag_data[rec_offset..rec_offset + rec_len]
                            .chunks_exact(2)
                            .map(|c| u16::from_be_bytes([c[0], c[1]]))
                            .collect();
                        if let Ok(desc) = String::from_utf16(&u16_data) {
                            description = desc.trim().to_string();
                            break;
                        }
                    }
                }
            }

            offset += 12;
        }
    }

    Ok(IccProfileInfo {
        description,
        color_space,
        device_class,
        version_major,
        version_minor,
        size: if declared_size > 0 { declared_size } else { data.len() },
    })
}

/// Reads and validates an ICC profile file from disk.
pub fn load_icc_profile_file<P: AsRef<Path>>(path: P) -> Result<IccProfileInfo, ColorManagementError> {
    let path_ref = path.as_ref();
    let data = std::fs::read(path_ref).map_err(|e| {
        ColorManagementError::IoError(format!("{}: {}", path_ref.display(), e))
    })?;
    parse_icc_profile(&data)
}

/// Generates a valid minimal synthetic ICC v2 profile in memory for test and fallback purposes.
pub fn generate_synthetic_icc_profile(name: &str) -> Vec<u8> {
    let name_bytes = name.as_bytes();
    let name_len = (name_bytes.len() + 1) as u32; // includes null terminator
    let tag_size = 12 + name_len;
    let total_size = 128 + 4 + 12 + tag_size;

    let mut buf = vec![0u8; total_size as usize];

    // Header (128 bytes)
    buf[0..4].copy_from_slice(&total_size.to_be_bytes()); // Size
    buf[4..8].copy_from_slice(b"crtn"); // CMM
    buf[8] = 2; // Version 2.4.0
    buf[9] = 0x40;
    buf[12..16].copy_from_slice(b"mntr"); // Display
    buf[16..20].copy_from_slice(b"RGB "); // RGB
    buf[20..24].copy_from_slice(b"XYZ "); // PCS
    buf[36..40].copy_from_slice(b"acsp"); // Magic signature

    // Tag table: 1 tag
    buf[128..132].copy_from_slice(&1u32.to_be_bytes());
    let tag_offset = 128 + 4 + 12;
    buf[132..136].copy_from_slice(b"desc");
    buf[136..140].copy_from_slice(&(tag_offset as u32).to_be_bytes());
    buf[140..144].copy_from_slice(&tag_size.to_be_bytes());

    // Tag data: textDescriptionType
    let tag_start = tag_offset as usize;
    buf[tag_start..tag_start + 4].copy_from_slice(b"desc");
    buf[tag_start + 8..tag_start + 12].copy_from_slice(&name_len.to_be_bytes());
    buf[tag_start + 12..tag_start + 12 + name_bytes.len()].copy_from_slice(name_bytes);

    buf
}
