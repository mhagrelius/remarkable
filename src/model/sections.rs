//! Cutting a tall page into sections the model can actually read.
//!
//! A reMarkable notebook exports as one PNG per notebook, not per page: the
//! three samples this was built against are 2,160, 9,176 and 17,002 pixels
//! tall at the same 1,620 wide.
//!
//! How much cutting them up is worth is an open question — see *The problem the
//! app actually has* in DESIGN.md. On the three samples a single request scores
//! the same, because llama-server caps every image at ~4,000 tokens whatever
//! its size and reads the whole notebook anyway. What splitting still buys is a
//! bound on the output length, which a notebook long enough to exhaust one
//! response would need. Measure before removing it, and measure before
//! elaborating it.
//!
//! Two rules decide where a cut goes:
//!
//! * No section is taller than [`Layout::max_section_height`] for the page's width,
//!   because that is the ceiling above which quality falls off.
//! * A cut lands in whitespace, never through a line of writing. Handwriting
//!   sliced in half mid-x-height is not recoverable by either neighbour.
//!
//! Adjacent sections overlap, so a line near a boundary is transcribed twice and
//! [`super::merge`] drops the copy. Overlapping costs a re-read; not
//! overlapping costs the line.
//!
//! Everything here is arithmetic over a row-luminance profile — one byte per
//! image row, the mean brightness of that row. No image type appears, which is
//! what lets these tests run with no display and no decoder.

/// Where the cuts go and how much sections overlap.
///
/// Both lengths here are **relative to the page's width**, not absolute pixel
/// counts, and that is deliberate. A section should hold about one page of
/// writing, and a gap should be about one line of writing tall; neither of
/// those is a number of pixels, because the same notebook scanned at twice the
/// resolution is the same notebook. Pinning them to pixels means every one has
/// to be found and changed together whenever the resolution moves — which was
/// missed once already, and quietly shrank what counted as a paragraph break
/// until a line went missing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The tallest a section may be, as a multiple of the page's width. Sections are
    /// made smaller when the whitespace allows; none is ever taller.
    pub max_section_aspect: f32,
    /// How much of the page is spent on overlap, as a percentage, shared out
    /// across the boundaries.
    pub overlap_percent: f32,
    /// A row this bright or brighter counts as blank. Not 255: the exports are
    /// antialiased and carry a faint ruled grid, so a genuinely empty row
    /// averages in the high 240s rather than pure white.
    pub blank_threshold: u8,
    /// The shortest run of blank rows that is a gap worth cutting in, as a
    /// fraction of the page's width. Below this it is the space between two
    /// lines of writing, not a paragraph break.
    pub min_gap_ratio: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            // Close to the shape of a reMarkable page, which is what the prompt
            // was written for.
            max_section_aspect: 1.4,
            overlap_percent: 18.0,
            blank_threshold: 246,
            // A line of this handwriting is a little over 1/70th of the page
            // width tall; a gap has to be at least that to be a real break.
            min_gap_ratio: 1.0 / 72.0,
        }
    }
}

impl Layout {
    /// The section-height ceiling for a page this wide.
    pub fn max_section_height(&self, width: u32) -> u32 {
        ((f64::from(width) * f64::from(self.max_section_aspect)) as u32).max(1)
    }

    /// The shortest cuttable gap on a page this wide.
    pub fn min_gap(&self, width: u32) -> u32 {
        ((f64::from(width) * f64::from(self.min_gap_ratio)) as u32).max(1)
    }
}

/// One horizontal slice of a page, in source rows. Half-open: `top..bottom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub top: u32,
    pub bottom: u32,
    pub index: usize,
    pub total: usize,
}

impl Section {
    pub fn height(&self) -> u32 {
        self.bottom - self.top
    }

    /// Whether this section shares rows with the one above it.
    pub fn overlaps_above(&self) -> bool {
        self.index > 0
    }

    /// Whether this section shares rows with the one below it.
    pub fn overlaps_below(&self) -> bool {
        self.index + 1 < self.total
    }
}

/// Runs of blank rows, as half-open `start..end` ranges.
///
/// A run shorter than `min_gap` is not reported: the gap between two lines of
/// the same paragraph is blank too, and cutting there would separate a wrapped
/// list item from its bullet.
pub fn blank_runs(profile: &[u8], threshold: u8, min_gap: u32) -> Vec<(u32, u32)> {
    let mut runs = Vec::new();
    let mut start: Option<u32> = None;

    for (row, brightness) in profile.iter().enumerate() {
        let row = row as u32;
        match (*brightness >= threshold, start) {
            (true, None) => start = Some(row),
            (false, Some(from)) => {
                if row - from >= min_gap {
                    runs.push((from, row));
                }
                start = None;
            }
            _ => {}
        }
    }

    // A page that ends in whitespace, which most do.
    if let Some(from) = start {
        let end = profile.len() as u32;
        if end - from >= min_gap {
            runs.push((from, end));
        }
    }

    runs
}

/// How many sections a page of this height needs.
///
/// Middle sections are the constraint. They carry overlap on both sides, so they
/// are the tallest, and it is their height that has to fit under the ceiling.
fn section_count(height: u32, ceiling: u32, layout: &Layout) -> usize {
    if height <= ceiling {
        return 1;
    }

    let height = f64::from(height);
    let ceiling = f64::from(ceiling);
    let overlap = f64::from(layout.overlap_percent) / 100.0;

    // Walk up from two until the worst-case section fits. Bounded rather than
    // solved for: the closed form is only marginally shorter and this cannot
    // run away on a pathological aspect ratio.
    for n in 2..=64usize {
        let base = height / n as f64;
        let per_boundary = height * overlap / (n - 1) as f64;
        if base + 2.0 * per_boundary <= ceiling {
            return n;
        }
    }
    64
}

/// Where to cut, given the blank rows.
///
/// Cuts are wanted at even intervals; they end up at the nearest blank run to
/// each of those, within a search radius. When nothing blank is close enough
/// the ideal position is used as-is — a cut through writing is bad, but a section
/// over the height ceiling is worse, and the overlap gives the neighbour a
/// chance at the severed line either way.
pub fn cut_points(blank: &[(u32, u32)], height: u32, ceiling: u32, layout: &Layout) -> Vec<u32> {
    let count = section_count(height, ceiling, layout);
    if count < 2 {
        return Vec::new();
    }

    let radius = f64::from(height) * 0.15;
    let mut cuts: Vec<u32> = Vec::with_capacity(count - 1);

    for i in 1..count {
        let ideal = f64::from(height) * i as f64 / count as f64;
        let nearest = blank
            .iter()
            .map(|(from, to)| from + (to - from) / 2)
            // Never cut where the last cut already is, or a section comes out
            // empty and the model is asked to read nothing.
            .filter(|midpoint| cuts.last() != Some(midpoint))
            .map(|midpoint| (midpoint, (f64::from(midpoint) - ideal).abs()))
            .filter(|(_, distance)| *distance <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(midpoint, _)| midpoint);

        cuts.push(nearest.unwrap_or(ideal as u32));
    }

    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

/// The sections of a page, overlapping.
///
/// `profile` is one byte per row: the mean brightness of that row. `width` is
/// the page's width in the same pixels, which is what turns the layout's
/// ratios into lengths.
pub fn split(profile: &[u8], width: u32, layout: &Layout) -> Vec<Section> {
    let height = profile.len() as u32;
    if height == 0 || width == 0 {
        return Vec::new();
    }

    let ceiling = layout.max_section_height(width);
    let cuts = cut_points(
        &blank_runs(profile, layout.blank_threshold, layout.min_gap(width)),
        height,
        ceiling,
        layout,
    );

    if cuts.is_empty() {
        return vec![Section {
            top: 0,
            bottom: height,
            index: 0,
            total: 1,
        }];
    }

    let total = cuts.len() + 1;
    // The overlap budget for the whole page, shared across the boundaries.
    let reach =
        (f64::from(height) * f64::from(layout.overlap_percent) / 100.0 / (total - 1) as f64) as u32;

    (0..total)
        .map(|index| {
            let base_top = if index == 0 { 0 } else { cuts[index - 1] };
            let base_bottom = cuts.get(index).copied().unwrap_or(height);

            // Reach past the cut into the neighbour, but never past the page.
            let mut top = base_top.saturating_sub(reach);
            let mut bottom = (base_bottom + reach).min(height);

            // Reaching both ways can push a middle section over the ceiling. Give
            // back the excess, from whichever sides were extended.
            let excess = (bottom - top).saturating_sub(ceiling);
            if excess > 0 {
                match (index == 0, index + 1 == total) {
                    (true, _) => bottom -= excess,
                    (_, true) => top += excess,
                    _ => {
                        top += excess / 2;
                        bottom -= excess - excess / 2;
                    }
                }
            }

            Section {
                top,
                bottom,
                index,
                total,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width every test page is treated as. Its only job is to turn the
    /// layout's ratios into the row counts the old absolute defaults had.
    const WIDTH: u32 = 1620;

    /// A page profile: `ink` rows of writing, then `gap` rows of white, over
    /// and over.
    fn ruled(lines: usize, ink: usize, gap: usize) -> Vec<u8> {
        let mut profile = Vec::new();
        for _ in 0..lines {
            profile.extend(std::iter::repeat_n(180u8, ink));
            profile.extend(std::iter::repeat_n(252u8, gap));
        }
        profile
    }

    #[test]
    fn a_page_that_fits_is_left_whole() {
        let layout = Layout::default();
        let page = ruled(20, 40, 30); // 1,400 rows
        let sections = split(&page, WIDTH, &layout);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].top, 0);
        assert_eq!(sections[0].bottom, page.len() as u32);
        assert!(!sections[0].overlaps_above() && !sections[0].overlaps_below());
    }

    #[test]
    fn no_section_is_taller_than_the_ceiling() {
        let layout = Layout::default();
        // The tall sample: 17,002 rows.
        let page = ruled(243, 40, 30);
        let sections = split(&page, WIDTH, &layout);
        assert!(sections.len() > 9, "got {} sections", sections.len());
        for section in &sections {
            assert!(
                section.height() <= layout.max_section_height(WIDTH),
                "section {} is {} rows",
                section.index,
                section.height()
            );
        }
    }

    #[test]
    fn the_sections_cover_the_whole_page() {
        let layout = Layout::default();
        let page = ruled(120, 40, 30); // 8,400 rows
        let sections = split(&page, WIDTH, &layout);

        assert_eq!(sections.first().expect("a section").top, 0);
        assert_eq!(
            sections.last().expect("a section").bottom,
            page.len() as u32
        );
        // Consecutive sections must not leave a strip of the page unread.
        for pair in sections.windows(2) {
            assert!(
                pair[1].top <= pair[0].bottom,
                "rows {}..{} belong to no section",
                pair[0].bottom,
                pair[1].top
            );
        }
    }

    #[test]
    fn neighbouring_sections_share_rows() {
        let layout = Layout::default();
        let page = ruled(120, 40, 30);
        let sections = split(&page, WIDTH, &layout);
        for pair in sections.windows(2) {
            assert!(
                pair[1].top < pair[0].bottom,
                "sections {} and {} do not overlap",
                pair[0].index,
                pair[1].index
            );
        }
    }

    #[test]
    fn a_cut_lands_in_whitespace_not_through_a_line() {
        let layout = Layout::default();
        let page = ruled(120, 40, 30);
        let cuts = cut_points(
            &blank_runs(&page, layout.blank_threshold, layout.min_gap(WIDTH)),
            page.len() as u32,
            layout.max_section_height(WIDTH),
            &layout,
        );
        assert!(!cuts.is_empty());
        for cut in cuts {
            assert!(
                page[cut as usize] >= layout.blank_threshold,
                "cut at row {cut} is through ink"
            );
        }
    }

    #[test]
    fn the_space_between_two_lines_is_not_a_cut_site() {
        // Six-row gaps, under the eighteen-row minimum: this page has writing
        // all the way down and nowhere comfortable to cut.
        let page = ruled(300, 40, 6);
        let runs = blank_runs(&page, 246, 18);
        assert!(runs.is_empty(), "found {} spurious gaps", runs.len());
    }

    #[test]
    fn a_page_with_nowhere_to_cut_is_still_cut() {
        // Falling back to an even split is right: a section over the ceiling
        // reads worse than a section that starts mid-sentence, and the overlap
        // gives the severed line to the neighbour as well.
        let layout = Layout::default();
        let page = ruled(300, 40, 6); // 13,800 rows, no usable gaps
        let sections = split(&page, WIDTH, &layout);
        assert!(sections.len() > 7);
        for section in &sections {
            assert!(section.height() <= layout.max_section_height(WIDTH));
            assert!(section.height() > 0);
        }
    }

    #[test]
    fn blank_runs_are_found_at_the_top_and_the_bottom() {
        let mut page = vec![255u8; 50];
        page.extend(std::iter::repeat_n(100u8, 100));
        page.extend(std::iter::repeat_n(255u8, 50));
        assert_eq!(blank_runs(&page, 246, 20), vec![(0, 50), (150, 200)]);
    }

    #[test]
    fn an_empty_page_produces_no_sections() {
        assert!(split(&[], WIDTH, &Layout::default()).is_empty());
    }

    #[test]
    fn every_section_knows_where_it_sits() {
        let page = ruled(120, 40, 30);
        let sections = split(&page, WIDTH, &Layout::default());
        let total = sections.len();
        assert!(!sections[0].overlaps_above());
        assert!(sections[0].overlaps_below());
        assert!(sections[total - 1].overlaps_above());
        assert!(!sections[total - 1].overlaps_below());
        for (i, section) in sections.iter().enumerate() {
            assert_eq!(section.index, i);
            assert_eq!(section.total, total);
        }
    }
}
