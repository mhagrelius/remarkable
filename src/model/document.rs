//! Turning a PDF into pages of pixels.
//!
//! A reMarkable notebook downloaded from the tablet arrives as a PDF, and so
//! does an export from the web app that was not saved as PNG, so this is on the
//! main road rather than a convenience.
//!
//! poppler-utils, shelled out to, as in familiar. `pdftoppm` is on every
//! desktop that has a PDF viewer, it is the same renderer GNOME's own document
//! viewer uses, and the alternative — linking poppler-glib — adds a build
//! dependency to render an image this can get by asking for it. Nothing here
//! runs a process: it builds argument vectors and reads what comes back, so the
//! quoting and the parsing are testable without poppler installed.

use gtk::glib;

/// Whether these bytes are a PDF, by their magic number rather than their name.
pub fn is_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// `pdfinfo`, to find out how many pages there are before rendering any.
pub fn page_count_command(path: &std::path::Path) -> Vec<std::ffi::OsString> {
    vec!["pdfinfo".into(), path.as_os_str().to_owned()]
}

/// Read the page count out of `pdfinfo`'s output.
pub fn parse_page_count(output: &str) -> Option<usize> {
    output.lines().find_map(|line| {
        line.strip_prefix("Pages:")
            .and_then(|count| count.trim().parse().ok())
    })
}

/// `pdftoppm`, rendering every page to `<prefix>-N.png`.
///
/// `-scale-to-x` with `-scale-to-y -1` scales to a width and keeps the aspect
/// ratio. A DPI would give a different pixel size for every page geometry, and
/// a reMarkable page is not A4; the caller picks the width from the model's
/// token budget instead.
///
/// `-r` is deliberately not used, and neither is `-gray`: the exports carry a
/// pale ruled grid that reads as noise in grayscale at this scale.
pub fn render_command(
    path: &std::path::Path,
    prefix: &std::path::Path,
    width: i32,
) -> Vec<std::ffi::OsString> {
    vec![
        "pdftoppm".into(),
        "-png".into(),
        "-scale-to-x".into(),
        width.to_string().into(),
        "-scale-to-y".into(),
        "-1".into(),
        path.as_os_str().to_owned(),
        prefix.as_os_str().to_owned(),
    ]
}

/// Sort the files `pdftoppm` wrote into page order.
///
/// It pads the number to the width of the page count — `notes-01.png` for a
/// ten-page document, `notes-1.png` for a nine-page one — so sorting the names
/// as strings is right only by accident. The number is parsed out instead.
pub fn in_page_order(mut files: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    files.sort_by_key(|path| page_number(path).unwrap_or(usize::MAX));
    files
}

fn page_number(path: &std::path::Path) -> Option<usize> {
    path.file_stem()?
        .to_str()?
        .rsplit_once('-')
        .and_then(|(_, number)| number.parse().ok())
}

/// Render every page of a PDF, as pages ready to read.
///
/// This is the one function in `model/` that runs a process. It lives here
/// rather than in `ui/` for the same reason familiar's document reader does:
/// it needs no display and no widgets, so putting it here is what lets a test
/// drive real poppler over a real PDF.
pub fn rasterise(bytes: &[u8]) -> Result<Vec<super::raster::Page>, String> {
    use super::raster::Page;

    if !is_pdf(bytes) {
        return Err("That file is not a PDF.".into());
    }

    let directory = scratch().map_err(|error| format!("Could not make room to render: {error}"))?;

    // Decoding happens inside, before the cleanup: the pages are files on disk
    // until `Page::decode` has read them, and deleting the directory first
    // leaves nothing to read.
    let outcome = render_into(&directory, bytes).and_then(|paths| {
        let pages: Vec<Page> = paths
            .iter()
            .filter_map(|path| std::fs::read(path).ok())
            .filter_map(|bytes| Page::decode(&bytes).ok())
            .collect();
        if pages.is_empty() {
            Err("That PDF rendered no pages.".into())
        } else {
            Ok(pages)
        }
    });

    // Whatever happened, a notebook's worth of PNGs is not left in the cache.
    let _ = std::fs::remove_dir_all(&directory);
    outcome
}

/// Write the PDF into `directory`, run `pdftoppm`, and hand back what it wrote
/// in page order. The files are still on disk when this returns.
fn render_into(
    directory: &std::path::Path,
    bytes: &[u8],
) -> Result<Vec<std::path::PathBuf>, String> {
    let source = directory.join("notebook.pdf");
    std::fs::write(&source, bytes).map_err(|error| format!("Could not write the PDF: {error}"))?;

    // A PDF is vector, so there is no native resolution to preserve — this
    // picks the size at which a section of the usual shape exactly fills the
    // model's token budget.
    let width = super::raster::render_width(super::sections::Layout::default().max_section_aspect);
    let command = render_command(&source, &directory.join("page"), width);
    let output = std::process::Command::new(&command[0])
        .args(&command[1..])
        .output()
        .map_err(|error| format!("Reading a PDF needs pdftoppm, from poppler-utils ({error})."))?;

    if !output.status.success() {
        return Err(format!(
            "pdftoppm could not read that PDF: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let rendered: Vec<std::path::PathBuf> = std::fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect();

    Ok(in_page_order(rendered))
}

/// A directory of our own to render into, under the cache.
fn scratch() -> std::io::Result<std::path::PathBuf> {
    let base = glib::user_cache_dir().join("remarkable");
    std::fs::create_dir_all(&base)?;
    // The process id keeps two copies of the app from rendering over each
    // other; the counter keeps one copy from doing so to itself.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let nth = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let unique = base.join(format!("render-{}-{nth}", std::process::id()));
    let _ = std::fs::remove_dir_all(&unique);
    std::fs::create_dir(&unique)?;
    Ok(unique)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_pdf_is_recognised_by_its_bytes_not_its_name() {
        assert!(is_pdf(b"%PDF-1.7\n..."));
        assert!(!is_pdf(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_pdf(b""));
    }

    #[test]
    fn the_page_count_is_read_out_of_pdfinfo() {
        let output = "Title:          Notes\nPages:          17\nPage size:      445 x 594 pts\n";
        assert_eq!(parse_page_count(output), Some(17));
    }

    #[test]
    fn output_without_a_page_count_is_none_rather_than_a_guess() {
        assert_eq!(parse_page_count("Syntax Error: Couldn't read xref\n"), None);
        assert_eq!(parse_page_count(""), None);
        assert_eq!(parse_page_count("Pages:          many"), None);
    }

    #[test]
    fn rendering_asks_for_a_width_and_lets_the_height_follow() {
        let command = render_command(
            Path::new("/tmp/notes.pdf"),
            Path::new("/tmp/out/page"),
            1288,
        );
        assert_eq!(command[0], "pdftoppm");
        assert!(command.contains(&"-png".into()));
        // -1 for the height is what makes -scale-to-x preserve the aspect.
        let width_at = command
            .iter()
            .position(|a| a == "-scale-to-x")
            .expect("a width");
        assert_eq!(command[width_at + 1], "1288");
        let height_at = command
            .iter()
            .position(|a| a == "-scale-to-y")
            .expect("a height");
        assert_eq!(command[height_at + 1], "-1");
    }

    #[test]
    fn a_path_with_a_space_or_a_quote_stays_one_argument() {
        let command = render_command(
            Path::new("/tmp/my notes \"final\".pdf"),
            Path::new("/tmp/out/page"),
            1288,
        );
        assert!(command.contains(&"/tmp/my notes \"final\".pdf".into()));
        assert_eq!(command.len(), 8);
    }

    #[test]
    fn pages_are_ordered_by_their_number_not_by_their_name() {
        // Nine pages: no padding, so a string sort puts 10 before 2 — except
        // there is no 10 here, which is exactly the case that hides the bug.
        let files: Vec<PathBuf> = ["page-1.png", "page-10.png", "page-2.png", "page-9.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let ordered: Vec<String> = in_page_order(files)
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            ordered,
            ["page-1.png", "page-2.png", "page-9.png", "page-10.png"]
        );
    }

    #[test]
    fn zero_padded_pages_order_the_same_way() {
        let files: Vec<PathBuf> = ["page-01.png", "page-10.png", "page-02.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let ordered: Vec<String> = in_page_order(files)
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        assert_eq!(ordered, ["page-01.png", "page-02.png", "page-10.png"]);
    }

    #[test]
    fn a_file_that_is_not_a_rendered_page_sorts_last_rather_than_panicking() {
        let files: Vec<PathBuf> = ["stray.png", "page-1.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(in_page_order(files)[0].to_string_lossy(), "page-1.png");
    }
}
