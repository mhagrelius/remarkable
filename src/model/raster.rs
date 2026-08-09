//! Pixels: decoding a page, measuring its rows, cutting out a section.
//!
//! gdk-pixbuf rather than an image crate. It is already in the process — GTK
//! decodes every icon through it — it handles the PNG and JPEG a reMarkable
//! export or a phone photo arrives as, and like cairo in familiar's document
//! writers it is not a GTK type and needs no display, so what is here is
//! testable in the display-free half of the suite.
//!
//! The only interesting operation is [`Page::profile`], which reduces a page to
//! one byte per row. That is all [`super::sections`] needs to find the gaps in
//! the writing, and reducing first means the splitter never sees a pixel and
//! stays pure arithmetic.

use gdk_pixbuf::prelude::*;
use gdk_pixbuf::{Colorspace, Pixbuf, PixbufLoader};

use super::sections::Section;

/// A page, exactly as it was decoded. Nothing here resamples it.
pub struct Page {
    pixbuf: Pixbuf,
}

/// Its size, and nothing about its contents — a page is a few megabytes of
/// pixels and printing them helps nobody.
impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Page({}x{})", self.width(), self.height())
    }
}

/// How many pixels the projector folds into one token.
///
/// From the mmproj metadata: `clip.vision.patch_size` 16 and
/// `clip.vision.spatial_merge_size` 2, so a token covers 32×32.
const PIXELS_PER_TOKEN: u32 = 32 * 32;

/// The most image tokens one request may spend.
///
/// **Measured, not chosen.** llama-server caps the image itself and silently
/// resamples anything over its budget. Sending one section at a range of sizes and
/// reading `usage.prompt_tokens` back:
///
/// | sent        |    MP | prompt_tokens |
/// |-------------|-------|---------------|
/// | 1288 × 1824 |   2.3 |         2,304 |
/// | 1620 × 2294 |   3.7 |         3,696 |
/// | 2048 × 2900 |   5.9 |     **4,052** |
/// | 2560 × 3625 |   9.3 |     **4,052** |
/// | 3240 × 4588 |  14.9 |     **4,052** |
///
/// The plateau at 4,052 is the cap. That one number explains every resolution
/// result this app has: 1,288 read acronyms worse because it genuinely sent
/// less detail, and 2,048 and up read them worse because the server downscaled
/// them *after* they had already been scaled here — a lossy round trip. The
/// reMarkable's native 1,620 won only by happening to land just under the cap.
///
/// So the rule is not a width. It is: **send every section untouched, unless it
/// would cross this budget, and only then scale it — once, here, where the
/// filter is known — to land just under.** Under the budget nothing is
/// resampled at all, whatever the page came from.
///
/// The cap applies to the *image*, not to the request: a real section plus the
/// full instruction measures 4,714 prompt tokens, of which 1,090 are the text.
/// The remaining 3,624 match `ceil(1620/32) · ceil(2267/32)` to within
/// rounding, and sit under the cap — so nothing is resampled at either end.
///
/// Set below the measured ceiling so rounding never pushes a section over it.
const MAX_IMAGE_TOKENS: u32 = 3_800;

/// The most pixels a section may carry before it has to be scaled down.
pub const MAX_SECTION_PIXELS: u32 = MAX_IMAGE_TOKENS * PIXELS_PER_TOKEN;

/// A sensible width to *render* at, for sources that have no native resolution.
///
/// A PDF is vector: there is no "leave it alone", something has to pick a size.
/// This picks the one where a section of the usual shape exactly fills the token
/// budget, so a rendered page gets all the detail the model can accept and not
/// a pixel more.
pub fn render_width(section_aspect: f32) -> i32 {
    let pixels = f64::from(MAX_SECTION_PIXELS);
    let aspect = f64::from(section_aspect).max(0.1);
    (pixels / aspect).sqrt() as i32
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RasterError {
    /// The bytes are not an image any installed loader recognises.
    Undecodable(String),
    /// A zero-width or zero-height image, or one so large the scale overflows.
    Unusable(String),
}

impl std::fmt::Display for RasterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undecodable(detail) => write!(f, "that file is not an image ({detail})"),
            Self::Unusable(detail) => write!(f, "that image cannot be read ({detail})"),
        }
    }
}

impl Page {
    /// Decode an image. Nothing is resampled here.
    ///
    /// A page is kept exactly as it arrived, however big, because the splitter
    /// wants the finest row profile it can get to place its cuts and because
    /// resampling twice is worse than resampling once. Whether a *section* has to
    /// be scaled to fit the model's budget is decided in [`Page::section_png`],
    /// which is the only place that knows how large the thing being sent is.
    pub fn decode(bytes: &[u8]) -> Result<Self, RasterError> {
        let loader = PixbufLoader::new();
        loader
            .write(bytes)
            .and_then(|()| loader.close())
            .map_err(|error| RasterError::Undecodable(error.message().to_string()))?;

        let pixbuf = loader
            .pixbuf()
            .ok_or_else(|| RasterError::Undecodable("no image in the file".into()))?;

        Self::from_pixbuf(pixbuf)
    }

    pub fn from_pixbuf(pixbuf: Pixbuf) -> Result<Self, RasterError> {
        let (width, height) = (pixbuf.width(), pixbuf.height());
        if width <= 0 || height <= 0 {
            return Err(RasterError::Unusable(format!("{width}x{height}")));
        }
        Ok(Self { pixbuf })
    }

    pub fn width(&self) -> u32 {
        self.pixbuf.width() as u32
    }

    pub fn height(&self) -> u32 {
        self.pixbuf.height() as u32
    }

    /// The mean brightness of each row, one byte per row.
    ///
    /// Brightness rather than a proper luma: the exports are black ink on white
    /// with a pale grey rule, so the three channels agree to within a few
    /// counts and weighting them would move no cut. Alpha is folded in as white
    /// — a transparent row is empty, and treating it as black would make every
    /// margin look like writing.
    pub fn profile(&self) -> Vec<u8> {
        let width = self.pixbuf.width() as usize;
        let height = self.pixbuf.height() as usize;
        let stride = self.pixbuf.rowstride() as usize;
        let channels = self.pixbuf.n_channels() as usize;
        let has_alpha = self.pixbuf.has_alpha();

        // Safety: the pixels live as long as the pixbuf, which this owns, and
        // nothing here writes to them or hands the slice out.
        let pixels = unsafe { self.pixbuf.pixels() };

        (0..height)
            .map(|y| {
                let row = &pixels[y * stride..y * stride + width * channels];
                let total: u64 = row
                    .chunks_exact(channels)
                    .map(|pixel| {
                        let ink = u64::from(pixel[0]) + u64::from(pixel[1]) + u64::from(pixel[2]);
                        if has_alpha {
                            // Composite onto white: a pixel that is half
                            // transparent is half as dark.
                            let alpha = u64::from(pixel[3]);
                            (ink * alpha + 3 * 255 * (255 - alpha)) / 255
                        } else {
                            ink
                        }
                    })
                    .sum();
                (total / (3 * width as u64)) as u8
            })
            .collect()
    }

    /// Cut out a section, as the PNG bytes that go on the wire.
    pub fn section_png(&self, section: &Section) -> Result<Vec<u8>, RasterError> {
        let height = self.height();
        let top = section.top.min(height) as i32;
        let bottom = section.bottom.min(height) as i32;
        if bottom <= top {
            return Err(RasterError::Unusable(format!(
                "section {} covers no rows",
                section.index
            )));
        }

        let slice = if top == 0 && bottom == height as i32 {
            self.pixbuf.clone()
        } else {
            let slice = Pixbuf::new(
                Colorspace::Rgb,
                self.pixbuf.has_alpha(),
                8,
                self.pixbuf.width(),
                bottom - top,
            )
            .ok_or_else(|| RasterError::Unusable("out of memory cutting a section".into()))?;
            self.pixbuf
                .copy_area(0, top, self.pixbuf.width(), bottom - top, &slice, 0, 0);
            slice
        };

        fit_to_budget(&slice)?
            .save_to_bufferv("png", &[])
            .map_err(|error| RasterError::Unusable(error.message().to_string()))
    }
}

/// Scale a section down if — and only if — it would cross the model's token
/// budget.
///
/// Doing it here rather than letting llama-server do it is the whole point: the
/// server's own downscale is invisible, unlogged and applied on top of whatever
/// this had already done. One resample, with a filter chosen for line art, is
/// strictly better than two.
fn fit_to_budget(section: &Pixbuf) -> Result<Pixbuf, RasterError> {
    let (width, height) = (section.width(), section.height());
    let pixels = i64::from(width) * i64::from(height);
    let budget = i64::from(MAX_SECTION_PIXELS);
    if pixels <= budget {
        return Ok(section.clone());
    }

    // Both sides shrink by the same factor, so the aspect — and the shape of
    // the writing — is unchanged.
    let factor = (budget as f64 / pixels as f64).sqrt();
    let scaled_width = ((f64::from(width) * factor) as i32).max(1);
    let scaled_height = ((f64::from(height) * factor) as i32).max(1);

    section
        .scale_simple(
            scaled_width,
            scaled_height,
            // Hyper, not Bilinear: this only ever runs when shrinking, and bilinear
            // drops thin strokes between sample points — which on handwriting is
            // the ascender that tells a `1` from an `l`.
            gdk_pixbuf::InterpType::Hyper,
        )
        .ok_or_else(|| RasterError::Unusable("out of memory scaling a section".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page: `rows` tall, `width` wide, with a horizontal black stripe
    /// wherever `ink` says so.
    fn page(width: i32, ink: &[bool]) -> Page {
        let height = ink.len() as i32;
        let pixbuf = Pixbuf::new(Colorspace::Rgb, false, 8, width, height).expect("a pixbuf");
        pixbuf.fill(0xffff_ffff);

        let stride = pixbuf.rowstride() as usize;
        let pixels = unsafe { pixbuf.pixels() };
        for (y, dark) in ink.iter().enumerate() {
            if *dark {
                for x in 0..width as usize {
                    let at = y * stride + x * 3;
                    pixels[at..at + 3].fill(0);
                }
            }
        }
        Page::from_pixbuf(pixbuf).expect("a page")
    }

    #[test]
    fn a_profile_has_one_entry_per_row() {
        let page = page(64, &[false, true, false, true]);
        assert_eq!(page.profile(), vec![255, 0, 255, 0]);
    }

    #[test]
    fn transparency_reads_as_empty_not_as_ink() {
        let pixbuf = Pixbuf::new(Colorspace::Rgb, true, 8, 32, 2).expect("a pixbuf");
        // Black, fully transparent: an empty row, not the darkest one.
        pixbuf.fill(0x0000_0000);
        let page = Page::from_pixbuf(pixbuf).expect("a page");
        assert_eq!(page.profile(), vec![255, 255]);
    }

    #[test]
    fn a_page_is_never_resampled_however_big_it_is() {
        // Decoding keeps every pixel. Whether a *section* is too big for the model
        // is a separate question, answered in `section_png`.
        for width in [800, 1620, 4000] {
            let page = page(width, &[false; 300]);
            assert_eq!(page.width(), width as u32);
            assert_eq!(page.height(), 300);
        }
    }

    #[test]
    fn a_section_within_the_budget_is_sent_exactly_as_it_is() {
        // 1620 x 2268 is 3.7 MP, which is under the cap — the case that
        // matters, since it is what a reMarkable exports.
        let page = page(1620, &[false; 2268]);
        let png = page
            .section_png(&Section {
                top: 0,
                bottom: 2268,
                index: 0,
                total: 1,
            })
            .expect("a section");
        let sent = Page::decode(&png).expect("decodes");
        assert_eq!((sent.width(), sent.height()), (1620, 2268));
    }

    #[test]
    fn a_section_over_the_budget_is_scaled_down_to_fit_it() {
        // A phone photo of a page: far more pixels than the model will accept.
        let page = page(4000, &[false; 5600]);
        let png = page
            .section_png(&Section {
                top: 0,
                bottom: 5600,
                index: 0,
                total: 1,
            })
            .expect("a section");
        let sent = Page::decode(&png).expect("decodes");

        assert!(
            sent.width() * sent.height() <= MAX_SECTION_PIXELS,
            "{}x{} is over the budget",
            sent.width(),
            sent.height()
        );
        // Close to the budget rather than far under it: scaling further than
        // necessary is the mistake this whole arrangement exists to avoid.
        assert!(sent.width() * sent.height() > MAX_SECTION_PIXELS * 9 / 10);
        // And the shape is unchanged, or the writing would be squashed.
        let before = 5600.0 / 4000.0;
        let after = f64::from(sent.height()) / f64::from(sent.width());
        assert!(
            (before - after).abs() < 0.01,
            "aspect changed: {before} -> {after}"
        );
    }

    #[test]
    fn the_render_width_puts_a_section_right_at_the_budget() {
        let width = render_width(1.4);
        let height = (f64::from(width) * 1.4) as u32;
        let pixels = u32::try_from(width).expect("positive") * height;
        assert!(pixels <= MAX_SECTION_PIXELS, "{pixels} over budget");
        assert!(
            pixels > MAX_SECTION_PIXELS * 9 / 10,
            "{pixels} wastefully under"
        );
    }

    #[test]
    fn a_section_is_cut_to_the_rows_it_names() {
        let page = page(64, &[false; 200]);
        let png = page
            .section_png(&Section {
                top: 40,
                bottom: 90,
                index: 1,
                total: 3,
            })
            .expect("a section");
        let decoded = Page::decode(&png).expect("decodes");
        assert_eq!(decoded.height(), 50);
        assert_eq!(decoded.width(), 64);
    }

    #[test]
    fn a_section_running_past_the_bottom_is_clamped_to_the_page() {
        let page = page(64, &[false; 100]);
        let png = page
            .section_png(&Section {
                top: 80,
                bottom: 400,
                index: 1,
                total: 2,
            })
            .expect("a section");
        assert_eq!(Page::decode(&png).expect("decodes").height(), 20);
    }

    #[test]
    fn a_section_that_covers_nothing_is_an_error_not_an_empty_png() {
        let page = page(64, &[false; 100]);
        assert!(page
            .section_png(&Section {
                top: 100,
                bottom: 100,
                index: 0,
                total: 1,
            })
            .is_err());
    }

    #[test]
    fn a_section_is_written_as_a_png_whatever_arrived() {
        let page = page(64, &[false; 40]);
        let png = page
            .section_png(&Section {
                top: 0,
                bottom: 40,
                index: 0,
                total: 1,
            })
            .expect("a section");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[test]
    fn something_that_is_not_an_image_does_not_decode() {
        assert!(matches!(
            Page::decode(b"this is a text file"),
            Err(RasterError::Undecodable(_))
        ));
    }

    #[test]
    fn a_round_trip_through_png_preserves_the_profile() {
        let ink: Vec<bool> = (0..120).map(|y| (30..60).contains(&y)).collect();
        let original = page(64, &ink);
        let png = original
            .section_png(&Section {
                top: 0,
                bottom: 120,
                index: 0,
                total: 1,
            })
            .expect("a section");
        assert_eq!(
            Page::decode(&png).expect("decodes").profile(),
            original.profile()
        );
    }
}
