//! Carrying a PDF's pictures into the EPUB.
//!
//! A picture in a PDF is a stream of samples plus a description of how to read
//! them, and the description is where the work is. Two encodings need no
//! decoding at all — a `DCTDecode` stream is already a JPEG file — while the
//! rest are raw samples that have to be turned back into pixels through a
//! colour space before anything can be written.
//!
//! What cannot be read is refused by name rather than guessed at, exactly as a
//! catalogue's offers are. A scanner's `JBIG2Decode` and `JPXDecode` pages are
//! the common case of that, and they are also the case where carrying the
//! picture would be pointless: a book whose pages are pictures converts to a
//! worse copy of the PDF it came from.
use super::text::Drawn;
use lopdf::{Document, Object, ObjectId, Stream};

/// A picture, ready to be written into the book.
pub(super) struct Picture {
    pub bytes: Vec<u8>,
    pub media_type: &'static str,
    pub extension: &'static str,
}

/// Enough pixels for any figure on a reader's screen, and a bound on what a
/// malformed stream can ask to be allocated.
const MAX_PIXELS: u64 = 40_000_000;
/// Smaller than this on the page and it is a rule, a bullet or a spacer rather
/// than a figure worth its own line in the book.
const SMALLEST_POINTS: f32 = 24.0;

/// A picture covering this much of the page in both directions is the page
/// itself rather than something on it.
const COVERS_THE_PAGE: f32 = 0.9;

/// Whether this is worth carrying at all, from its size on the page.
///
/// Two things are not figures. Anything tiny is a rule, a bullet or a spacer.
/// Anything filling the page is a scan of the page, and carrying those turns a
/// book into a heavier copy of the PDF it came from with none of the reasons
/// anyone wanted an EPUB.
pub(super) fn worth_carrying(drawn: &Drawn, page: (f32, f32)) -> bool {
    let big_enough = drawn.width >= SMALLEST_POINTS && drawn.height >= SMALLEST_POINTS;
    let is_the_page =
        drawn.width >= page.0 * COVERS_THE_PAGE && drawn.height >= page.1 * COVERS_THE_PAGE;
    big_enough && !is_the_page
}

/// The picture a name refers to, if it is a picture at all.
///
/// Forms are followed, because a producer may put the whole page inside one
/// and every picture on it is then named in the form's own resources.
pub(super) fn find(document: &Document, page: ObjectId, name: &[u8]) -> Option<Stream> {
    let (direct, streams) = document.get_page_resources(page).ok()?;
    let dictionaries = direct.into_iter().chain(
        streams
            .iter()
            .filter_map(|id| document.get_dictionary(*id).ok()),
    );
    dictionaries
        .into_iter()
        .find_map(|resources| named(document, resources, name, 0))
}

fn named(
    document: &Document,
    resources: &lopdf::Dictionary,
    name: &[u8],
    depth: usize,
) -> Option<Stream> {
    if depth >= 8 {
        return None;
    }
    let objects = resources
        .get(b"XObject")
        .ok()
        .and_then(|entry| document.dereference(entry).ok())
        .and_then(|(_, object)| object.as_dict().ok())?;
    let stream = |entry| {
        document
            .dereference(entry)
            .ok()
            .and_then(|(_, object)| object.as_stream().ok())
    };
    if let Some(found) = objects.get(name).ok().and_then(stream) {
        if found
            .dict
            .get(b"Subtype")
            .and_then(|s| s.as_name())
            .is_ok_and(|kind| kind == b"Image")
        {
            return Some(found.clone());
        }
    }
    // Not here: look inside the forms this page draws.
    objects.iter().find_map(|(_, entry)| {
        let form = stream(entry)?;
        let inner = form
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|entry| document.dereference(entry).ok())
            .and_then(|(_, object)| object.as_dict().ok())?;
        named(document, inner, name, depth + 1)
    })
}

/// The filters a stream is wrapped in, outermost first.
///
/// The entry may be written as a reference, and reading it as a name alone
/// would call a JPEG raw samples and rebuild it into noise.
fn filters(document: &Document, stream: &Stream) -> Vec<Vec<u8>> {
    let entry = stream
        .dict
        .get(b"Filter")
        .ok()
        .and_then(|object| document.dereference(object).ok())
        .map(|(_, object)| object);
    match entry {
        Some(Object::Name(name)) => vec![name.clone()],
        Some(Object::Array(items)) => items
            .iter()
            .filter_map(|item| {
                document
                    .dereference(item)
                    .ok()
                    .and_then(|(_, object)| object.as_name().ok().map(<[u8]>::to_vec))
            })
            .collect(),
        _ => vec![],
    }
}

/// Sample depths this reads back. Anything else is refused rather than
/// guessed at.
fn supported_depth(bits: u32) -> bool {
    matches!(bits, 1 | 2 | 4 | 8 | 16)
}

/// The shape of a picture, or nothing if it is one this cannot read.
///
/// One gate for both judging and decoding: if these disagreed, a book could
/// be called perfect and then come out with a picture quietly missing.
fn readable(document: &Document, stream: &Stream) -> Option<Shape> {
    let encoding = filters(document, stream);
    if encoding.iter().any(|name| {
        matches!(
            name.as_slice(),
            b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode"
        )
    }) {
        return None;
    }
    let width = number(stream.dict.get(b"Width").ok()?)?;
    let height = number(stream.dict.get(b"Height").ok()?)?;
    if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
        return None;
    }
    // A JPEG is carried whole, so its colour space and depth are the
    // decoder's problem rather than ours.
    if encoding.last().is_some_and(|name| name == b"DCTDecode") {
        return Some(Shape::Jpeg {
            outer: encoding.len() > 1,
        });
    }
    let bits = stream
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(number)
        .unwrap_or(8);
    supported_depth(bits).then_some(())?;
    Some(Shape::Samples {
        width,
        height,
        bits,
        space: components(document, stream)?,
    })
}

enum Shape {
    /// Already a JPEG file; `outer` says whether anything is wrapped round it.
    Jpeg { outer: bool },
    Samples {
        width: u32,
        height: u32,
        bits: u32,
        space: Space,
    },
}

/// Whether this picture can be carried, without decoding it.
///
/// Cheap on purpose: every PDF is judged during discovery, so the verdict must
/// not depend on decompressing every image in the book.
pub(super) fn carryable(document: &Document, stream: &Stream) -> bool {
    readable(document, stream).is_some()
}

/// How many samples each pixel has, for a colour space we can read back.
fn components(document: &Document, stream: &Stream) -> Option<Space> {
    let entry = stream
        .dict
        .get(b"ColorSpace")
        .ok()
        .and_then(|object| document.dereference(object).ok())
        .map(|(_, object)| object.clone())?;
    space(document, &entry, 0)
}

/// A colour space, reduced to what is needed to turn samples into pixels.
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// A palette: the base space's pixels, and the table to look them up in.
    Indexed(Box<Space>, Vec<u8>),
}

impl Space {
    fn samples(&self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
            Self::Indexed(..) => 1,
        }
    }
}

fn space(document: &Document, object: &Object, depth: usize) -> Option<Space> {
    if depth > 4 {
        return None;
    }
    match object {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" | b"CalGray" | b"G" => Some(Space::Gray),
            b"DeviceRGB" | b"CalRGB" | b"RGB" => Some(Space::Rgb),
            b"DeviceCMYK" | b"CMYK" => Some(Space::Cmyk),
            _ => None,
        },
        Object::Array(items) => {
            let first = items.first()?.as_name().ok()?;
            match first {
                // An ICC profile is not read; the component count it declares
                // says which device space it stands in for.
                b"ICCBased" => {
                    let stream = document
                        .dereference(items.get(1)?)
                        .ok()
                        .and_then(|(_, object)| object.as_stream().ok())?;
                    match stream.dict.get(b"N").ok().and_then(|n| n.as_i64().ok())? {
                        1 => Some(Space::Gray),
                        3 => Some(Space::Rgb),
                        4 => Some(Space::Cmyk),
                        _ => None,
                    }
                }
                b"Indexed" | b"I" => {
                    let base = space(
                        document,
                        &document.dereference(items.get(1)?).ok()?.1.clone(),
                        depth + 1,
                    )?;
                    let table = match document.dereference(items.get(3)?).ok()?.1 {
                        Object::String(bytes, _) => bytes.clone(),
                        Object::Stream(stream) => stream.decompressed_content().ok()?,
                        _ => return None,
                    };
                    Some(Space::Indexed(Box::new(base), table))
                }
                // A separation or device-N space paints through a tint
                // transform, which is a function this does not evaluate.
                _ => None,
            }
        }
        Object::Reference(_) => space(
            document,
            &document.dereference(object).ok()?.1.clone(),
            depth + 1,
        ),
        _ => None,
    }
}

/// Turn a picture into something an EPUB can hold.
pub(super) fn decode(document: &Document, stream: &Stream) -> Option<Picture> {
    match readable(document, stream)? {
        // A DCTDecode stream is a JPEG file already, so it is carried across
        // without being decoded and re-encoded, which would cost quality for
        // nothing.
        Shape::Jpeg { outer } => Some(Picture {
            // Any filters outside the JPEG still have to come off.
            bytes: if outer {
                unwrap_outer(document, stream)?
            } else {
                stream.content.clone()
            },
            media_type: "image/jpeg",
            extension: "jpg",
        }),
        Shape::Samples {
            width,
            height,
            bits,
            space,
        } => {
            let samples = stream.decompressed_content().ok()?;
            let pixels = to_rgb(&samples, width, height, bits, &space, inverted(stream))?;
            let alpha = soft_mask(document, stream, width, height);
            encode_png(&pixels, width, height, alpha.as_deref())
        }
    }
}

/// Strip every filter except the innermost image codec.
fn unwrap_outer(document: &Document, stream: &Stream) -> Option<Vec<u8>> {
    let mut copy = stream.clone();
    // Dropping the image codec from the chain leaves lopdf willing to undo the
    // compression wrapped around it.
    let remaining: Vec<Object> = filters(document, stream)
        .into_iter()
        .filter(|name| name != b"DCTDecode")
        .map(Object::Name)
        .collect();
    copy.dict.set("Filter", Object::Array(remaining));
    copy.decompressed_content().ok()
}

fn number(object: &Object) -> Option<u32> {
    object.as_i64().ok().and_then(|v| u32::try_from(v).ok())
}

/// Samples to RGB, which is the one form everything below can be written from.
fn to_rgb(
    samples: &[u8],
    width: u32,
    height: u32,
    bits: u32,
    space: &Space,
    inverted: bool,
) -> Option<Vec<u8>> {
    let per_pixel = space.samples();
    let mut out = Vec::with_capacity(width as usize * height as usize * 3);
    // Rows are padded to a byte boundary, which matters for anything under 8
    // bits: a 1-bit scan read without the padding shears by a pixel a row.
    let row_bits = width as usize * per_pixel * bits as usize;
    let row_bytes = row_bits.div_ceil(8);
    if samples.len() < row_bytes * height as usize {
        return None;
    }
    let max = ((1u32 << bits) - 1) as f32;
    for row in 0..height as usize {
        let start = row * row_bytes;
        for column in 0..width as usize {
            let mut values = [0u32; 4];
            for (component, value) in values.iter_mut().enumerate().take(per_pixel) {
                let index = column * per_pixel + component;
                *value = read(samples, start, index, bits)?;
                // A Decode array of [1 0] says the samples run the other way.
                // An index into a palette is a position, not a level, so it is
                // never turned around.
                if inverted && !matches!(space, Space::Indexed(..)) {
                    *value = (max as u32).saturating_sub(*value);
                }
            }
            match space {
                Space::Gray => {
                    let v = (values[0] as f32 / max * 255.0) as u8;
                    out.extend_from_slice(&[v, v, v]);
                }
                Space::Rgb => {
                    for value in values.iter().take(3) {
                        out.push((*value as f32 / max * 255.0) as u8);
                    }
                }
                Space::Cmyk => {
                    let c = values.map(|v| v as f32 / max);
                    for channel in 0..3 {
                        out.push((255.0 * (1.0 - c[channel]) * (1.0 - c[3])) as u8);
                    }
                }
                Space::Indexed(base, table) => {
                    let width = base.samples();
                    let at = values[0] as usize * width;
                    for component in 0..3 {
                        let value = match **base {
                            Space::Gray => table.get(at).copied().unwrap_or(0),
                            Space::Rgb => table.get(at + component).copied().unwrap_or(0),
                            Space::Cmyk => {
                                let c = |i: usize| {
                                    table.get(at + i).copied().unwrap_or(0) as f32 / 255.0
                                };
                                (255.0 * (1.0 - c(component)) * (1.0 - c(3))) as u8
                            }
                            Space::Indexed(..) => 0,
                        };
                        out.push(value);
                    }
                }
            }
        }
    }
    Some(out)
}

/// Whether the samples run from light to dark rather than dark to light.
fn inverted(stream: &Stream) -> bool {
    stream
        .dict
        .get(b"Decode")
        .ok()
        .and_then(|object| object.as_array().ok())
        .is_some_and(|range| {
            let value = |index: usize| {
                range
                    .get(index)
                    .and_then(|object| object.as_float().ok())
                    .unwrap_or(0.0)
            };
            value(0) > value(1)
        })
}

/// One sample, which below 8 bits is a run of bits inside a byte.
fn read(samples: &[u8], row: usize, index: usize, bits: u32) -> Option<u32> {
    match bits {
        8 => samples.get(row + index).map(|v| *v as u32),
        16 => {
            // Both bytes, or the value lands in 0..255 while the range it is
            // measured against is 0..65535 and every picture comes out black.
            let at = row + index * 2;
            Some((*samples.get(at)? as u32) << 8 | *samples.get(at + 1)? as u32)
        }
        1 | 2 | 4 => {
            let offset = index * bits as usize;
            let byte = *samples.get(row + offset / 8)?;
            let shift = 8 - bits as usize - (offset % 8);
            Some((byte as u32 >> shift) & ((1 << bits) - 1))
        }
        _ => None,
    }
}

/// A picture's transparency, as a greyscale image the same shape as it.
fn soft_mask(document: &Document, stream: &Stream, width: u32, height: u32) -> Option<Vec<u8>> {
    let mask = stream
        .dict
        .get(b"SMask")
        .ok()
        .and_then(|object| document.dereference(object).ok())
        .and_then(|(_, object)| object.as_stream().ok())?;
    let (w, h) = (
        number(mask.dict.get(b"Width").ok()?)?,
        number(mask.dict.get(b"Height").ok()?)?,
    );
    let bits = mask
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(number)
        .unwrap_or(8);
    let samples = mask.decompressed_content().ok()?;
    let gray = to_rgb(&samples, w, h, bits, &Space::Gray, inverted(mask))?;
    // A mask of a different size would need resampling; it is rare enough to
    // be left opaque rather than stretched badly.
    (w == width && h == height).then(|| gray.iter().step_by(3).copied().collect())
}

/// Write the pixels as a PNG, flattening transparency onto white.
///
/// A reader shows a book on paper, so compositing on white is what the page
/// would have looked like; keeping the alpha would leave a logo as a black
/// rectangle on a device that ignores it.
fn encode_png(pixels: &[u8], width: u32, height: u32, alpha: Option<&[u8]>) -> Option<Picture> {
    let mut flattened;
    let pixels = match alpha {
        None => pixels,
        Some(alpha) => {
            flattened = pixels.to_vec();
            for (index, pixel) in flattened.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let a = *alpha.get(index).unwrap_or(&255) as f32 / 255.0;
                for channel in pixel {
                    *channel = (*channel as f32 * a + 255.0 * (1.0 - a)) as u8;
                }
            }
            &flattened
        }
    };
    let image: image::RgbImage = image::ImageBuffer::from_raw(width, height, pixels.to_vec())?;
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .ok()?;
    Some(Picture {
        bytes,
        media_type: "image/png",
        extension: "png",
    })
}

/// What a page's pictures amount to, without decoding any of them.
#[derive(Clone, Copy, Default)]
pub(super) struct Verdict {
    pub any: bool,
    /// At least one picture will not make it into the book.
    pub dropped: bool,
}

/// Judge a page's pictures: cheap, because every PDF is judged on discovery.
pub(super) fn judge(document: &Document, page: ObjectId, drawn: &[Drawn]) -> Verdict {
    let size = super::text::size(document, page);
    let mut verdict = Verdict::default();
    for item in drawn {
        let Some(stream) = find(document, page, &item.name) else {
            continue;
        };
        verdict.any = true;
        if !worth_carrying(item, size) || !carryable(document, &stream) {
            // A rule or a spacer is not a loss, but it is also not big enough
            // to have been one; only a picture worth carrying counts.
            if item.width >= SMALLEST_POINTS && item.height >= SMALLEST_POINTS {
                verdict.dropped = true;
            }
        }
    }
    verdict
}

/// Every picture a page draws, in the order it draws them.
pub(super) fn on_page(document: &Document, page: ObjectId, drawn: &[Drawn]) -> Vec<(f32, Picture)> {
    let size = super::text::size(document, page);
    drawn
        .iter()
        .filter(|item| worth_carrying(item, size))
        .filter_map(|item| {
            let stream = find(document, page, &item.name)?;
            Some((item.top, decode(document, &stream)?))
        })
        .collect()
}
