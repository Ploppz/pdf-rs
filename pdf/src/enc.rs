use inflate::{inflate_bytes, inflate_bytes_zlib};
use itertools::Itertools;
use std::mem;

use crate as pdf;
use crate::error::*;
use crate::object::{Object, Resolve};
use crate::primitive::{Dictionary, Primitive};

#[derive(Object, Debug, Clone)]
pub struct LZWFlateParams {
    #[pdf(key = "Predictor", default = "1")]
    predictor: i32,
    #[pdf(key = "Colors", default = "1")]
    n_components: i32,
    #[pdf(key = "BitsPerComponent", default = "8")]
    bits_per_component: i32,
    #[pdf(key = "Columns", default = "1")]
    columns: i32,
    #[pdf(key = "EarlyChange", default = "1")]
    early_change: i32,
}

#[derive(Object, Debug, Clone)]
pub struct DCTDecodeParams {
    // TODO The default value of ColorTransform is 1 if the image has three components and 0 otherwise.
    // 0:   No transformation.
    // 1:   If the image has three color components, transform RGB values to YUV before encoding and from YUV to RGB after decoding.
    //      If the image has four components, transform CMYK values to YUVK before encoding and from YUVK to CMYK after decoding.
    //      This option is ignored if the image has one or two color components.
    #[pdf(key = "ColorTransform")]
    color_transform: Option<i32>,
}

#[derive(Debug, Clone)]
pub enum StreamFilter {
    ASCIIHexDecode,
    ASCII85Decode,
    LZWDecode(LZWFlateParams),
    FlateDecode(LZWFlateParams),
    JPXDecode, //Jpeg2k
    DCTDecode(DCTDecodeParams),
    CCITTFaxDecode,
    Crypt,
}
impl StreamFilter {
    pub fn from_kind_and_params(
        kind: &str,
        params: Dictionary,
        r: &impl Resolve,
    ) -> Result<StreamFilter> {
        let params = Primitive::Dictionary(params);
        Ok(match kind {
            "ASCIIHexDecode" => StreamFilter::ASCIIHexDecode,
            "ASCII85Decode" => StreamFilter::ASCII85Decode,
            "LZWDecode" => StreamFilter::LZWDecode(LZWFlateParams::from_primitive(params, r)?),
            "FlateDecode" => StreamFilter::FlateDecode(LZWFlateParams::from_primitive(params, r)?),
            "JPXDecode" => StreamFilter::JPXDecode,
            "DCTDecode" => StreamFilter::DCTDecode(DCTDecodeParams::from_primitive(params, r)?),
            "CCITTFaxDecode" => StreamFilter::CCITTFaxDecode,
            "Crypt" => StreamFilter::Crypt,
            ty => bail!("Unrecognized filter type {:?}", ty),
        })
    }
}

fn decode_nibble(c: u8) -> Option<u8> {
    match c {
        n @ b'0'..=b'9' => Some(n - b'0'),
        a @ b'a'..=b'h' => Some(a - b'a' + 0xa),
        a @ b'A'..=b'H' => Some(a - b'A' + 0xA),
        _ => None,
    }
}

pub fn decode_hex(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 2);
    for (i, (&high, &low)) in data.iter().tuples().enumerate() {
        if let (Some(low), Some(high)) = (decode_nibble(low), decode_nibble(high)) {
            out.push(high << 4 | low);
        } else {
            return Err(PdfError::HexDecode {
                pos: i * 2,
                bytes: [high, low],
            });
        }
    }
    Ok(out)
}

#[inline]
fn sym_85(byte: u8) -> Option<u8> {
    match byte {
        b @ 0x21..=0x75 => Some(b - 0x21),
        _ => None,
    }
}
fn word_85(input: &[u8]) -> Option<(u8, [u8; 4])> {
    match input.get(0).cloned() {
        Some(b'z') => Some((1, [0; 4])),
        Some(a) => {
            if let [b, c, d, e] = input[1..5] {
                let q: u32 = ((((sym_85(a)? as u32 * 85) + sym_85(b)? as u32 * 85)
                    + sym_85(c)? as u32 * 85)
                    + sym_85(d)? as u32 * 85)
                    + e as u32;
                Some((
                    5,
                    [(q >> 24) as u8, (q >> 16) as u8, (q >> 8) as u8, q as u8],
                ))
            } else {
                None
            }
        }
        None => None,
    }
}

fn substr(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|w| w == needle)
}

fn decode_85(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity((data.len() + 4 / 5) * 4);

    let mut pos = 0;
    while let Some((advance, word)) = word_85(&data[pos..]) {
        out.extend_from_slice(&word);
        pos += advance as usize;
    }
    let tail_len = substr(&data[pos..], b"~>").ok_or(PdfError::Ascii85TailError)?;
    assert!(tail_len < 5);
    if data.len() < pos + tail_len {
        return Err(PdfError::Ascii85TailError);
    }
    let mut tail = [b'u'; 5];
    tail[..tail_len].copy_from_slice(&data[pos..pos + tail_len]);

    let (_, last) = word_85(&tail).ok_or(PdfError::Ascii85TailError)?;
    out.extend_from_slice(&last[..tail_len - 1]);
    Ok(out)
}

fn flate_decode(data: &[u8], params: &LZWFlateParams) -> Result<Vec<u8>> {
    let predictor = params.predictor as usize;
    let n_components = params.n_components as usize;
    let columns = params.columns as usize;

    // First flate decode
    let decoded = match inflate_bytes_zlib(data) {
        Ok(data) => data,
        Err(_) => {
            info!("invalid zlib header. trying without");
            inflate_bytes(data)?
        }
    };
    // Then unfilter (PNG)
    // For this, take the old out as input, and write output to out

    if predictor > 10 {
        let inp = decoded; // input buffer
        let rows = inp.len() / (columns + 1);

        // output buffer
        let mut out = vec![0; rows * columns];

        // Apply inverse predictor
        let null_vec = vec![0; columns];

        let mut in_off = 0; // offset into input buffer

        let mut out_off = 0; // offset into output buffer
        let mut last_out_off = 0; // last offset to output buffer

        while in_off < inp.len() {
            let predictor = PredictorType::from_u8(inp[in_off])?;
            in_off += 1; // +1 because the first byte on each row is predictor

            let row_in = &inp[in_off..in_off + columns];
            let (prev_row, row_out) = if out_off == 0 {
                (&null_vec[..], &mut out[out_off..out_off + columns])
            } else {
                let (prev, curr) = out.split_at_mut(out_off);
                (&prev[last_out_off..], &mut curr[..columns])
            };
            unfilter(predictor, n_components, prev_row, row_in, row_out);

            last_out_off = out_off;

            in_off += columns;
            out_off += columns;
        }
        Ok(out)
    } else {
        Ok(decoded)
    }
}

fn dct_decode(data: &[u8], params: &DCTDecodeParams) -> Result<Vec<u8>> {
    use jpeg_decoder::Decoder;
    let mut decoder = Decoder::new(data);
    let pixels = decoder.decode()?;
    Ok(pixels)
}

pub fn decode(data: &[u8], filter: &StreamFilter) -> Result<Vec<u8>> {
    match *filter {
        StreamFilter::ASCIIHexDecode => decode_hex(data),
        StreamFilter::ASCII85Decode => decode_85(data),
        StreamFilter::FlateDecode(ref params) => flate_decode(data, params),
        StreamFilter::DCTDecode(ref params) => dct_decode(data, params),
        _ => unimplemented!(),
    }
}

/*
 * Predictor - copied and adapted from PNG crate..
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(dead_code)]
pub enum PredictorType {
    NoFilter = 0,
    Sub = 1,
    Up = 2,
    Avg = 3,
    Paeth = 4,
}

impl PredictorType {
    /// u8 -> Self. Temporary solution until Rust provides a canonical one.
    pub fn from_u8(n: u8) -> Result<PredictorType> {
        match n {
            0 => Ok(PredictorType::NoFilter),
            1 => Ok(PredictorType::Sub),
            2 => Ok(PredictorType::Up),
            3 => Ok(PredictorType::Avg),
            4 => Ok(PredictorType::Paeth),
            n => Err(PdfError::IncorrectPredictorType { n }.into()),
        }
    }
}

fn filter_paeth(a: u8, b: u8, c: u8) -> u8 {
    let ia = a as i16;
    let ib = b as i16;
    let ic = c as i16;

    let p = ia + ib - ic;

    let pa = (p - ia).abs();
    let pb = (p - ib).abs();
    let pc = (p - ic).abs();

    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

pub fn unfilter(filter: PredictorType, bpp: usize, prev: &[u8], inp: &[u8], out: &mut [u8]) {
    use self::PredictorType::*;
    let len = inp.len();
    assert_eq!(len, out.len());
    assert_eq!(len, prev.len());

    match filter {
        NoFilter => {
            for i in 0..len {
                out[i] = inp[i];
            }
        }
        Sub => {
            for i in bpp..len {
                out[i] = inp[i].wrapping_add(out[i - bpp]);
            }
        }
        Up => {
            for i in 0..len {
                out[i] = inp[i].wrapping_add(prev[i]);
            }
        }
        Avg => {
            for i in 0..bpp {
                out[i] = inp[i].wrapping_add(prev[i] / 2);
            }

            for i in bpp..len {
                out[i] = inp[i].wrapping_add(((out[i - bpp] as i16 + prev[i] as i16) / 2) as u8);
            }
        }
        Paeth => {
            for i in 0..bpp {
                out[i] = inp[i].wrapping_add(filter_paeth(0, prev[i], 0));
            }

            for i in bpp..len {
                out[i] = inp[i].wrapping_add(filter_paeth(out[i - bpp], prev[i], prev[i - bpp]));
            }
        }
    }
}

#[allow(unused)]
pub fn filter(method: PredictorType, bpp: usize, previous: &[u8], current: &mut [u8]) {
    use self::PredictorType::*;
    let len = current.len();

    match method {
        NoFilter => (),
        Sub => {
            for i in (bpp..len).rev() {
                current[i] = current[i].wrapping_sub(current[i - bpp]);
            }
        }
        Up => {
            for i in 0..len {
                current[i] = current[i].wrapping_sub(previous[i]);
            }
        }
        Avg => {
            for i in (bpp..len).rev() {
                current[i] =
                    current[i].wrapping_sub(current[i - bpp].wrapping_add(previous[i]) / 2);
            }

            for i in 0..bpp {
                current[i] = current[i].wrapping_sub(previous[i] / 2);
            }
        }
        Paeth => {
            for i in (bpp..len).rev() {
                current[i] = current[i].wrapping_sub(filter_paeth(
                    current[i - bpp],
                    previous[i],
                    previous[i - bpp],
                ));
            }

            for i in 0..bpp {
                current[i] = current[i].wrapping_sub(filter_paeth(0, previous[i], 0));
            }
        }
    }
}
