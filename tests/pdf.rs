//! The PDF road, driven over real poppler.
//!
//! A notebook fetched off the tablet arrives as a PDF, so this is the main road
//! and not a convenience. Everything else about PDFs is unit-tested by building
//! argument vectors and parsing output; this is the one place a `pdftoppm` is
//! actually run, because the failure worth catching — a flag poppler stopped
//! accepting, pages coming back in the wrong order — is invisible to a test
//! that only inspects the command.
//!
//! One `#[test]`, containing the cases. They share a scratch directory under
//! the cache and a process id, so a parallel run has one case's leftovers
//! looking like another case's failure to clean up.
//!
//! Skipped, loudly, where poppler is not installed. A missing tool is not a
//! failing build.

use remarkable::model::document;
use remarkable::model::raster::render_width;
use remarkable::model::sections::Layout;

/// A three-page PDF, written by hand.
///
/// Hand-built rather than committed as a fixture so the page count is visible
/// in the test that depends on it, and so there is no binary in the repo whose
/// contents nobody can read in a diff.
fn three_page_pdf() -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();

    objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count 3 >>",
        (0..3)
            .map(|n| format!("{} 0 R", 3 + n * 2))
            .collect::<Vec<_>>()
            .join(" ")
    ));

    for page in 0..3 {
        let stream = format!("BT /F1 48 Tf 72 500 Td (Page {}) Tj ET", page + 1);
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 \
             /BaseFont /Helvetica >> >> >> /Contents {} 0 R >>",
            4 + page * 2
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ));
    }

    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }

    let xref_at = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
    pdf.push_str("0000000000 65535 f \n");
    for offset in &offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    ));

    pdf.into_bytes()
}

fn poppler_installed() -> bool {
    std::process::Command::new("pdftoppm")
        .arg("-v")
        .output()
        .is_ok()
}

type Case = (&'static str, fn());

const CASES: &[Case] = &[
    ("a pdf becomes pages in order", pages_in_order),
    ("what is not a pdf is refused", refuses_other_files),
    ("a broken pdf says what poppler said", broken_pdf),
    ("rendering leaves nothing in the cache", leaves_no_litter),
];

#[test]
fn pdfs() {
    if !poppler_installed() {
        eprintln!("skipping: pdftoppm is not installed");
        return;
    }
    for (name, case) in CASES {
        eprintln!("  {name}");
        case();
    }
}

fn pages_in_order() {
    let pages = document::rasterise(&three_page_pdf()).expect("renders");

    assert_eq!(
        pages.len(),
        3,
        "a three-page PDF gave {} pages",
        pages.len()
    );
    // US Letter is 612x792 points. Derived rather than hardcoded, so changing
    // the budget does not silently make this assertion about nothing.
    let rendered = render_width(Layout::default().max_section_aspect);
    let width = u32::try_from(rendered).expect("a positive width");
    let expected = width * 792 / 612;

    for page in &pages {
        assert_eq!(page.width(), width);
        assert!(
            page.height().abs_diff(expected) <= 4,
            "612x792 at {width} wide should be ~{expected} tall, got {}",
            page.height()
        );
        // Every page has ink on it, so no row profile is uniformly blank.
        let profile = page.profile();
        assert_eq!(profile.len(), page.height() as usize);
        assert!(
            profile.iter().any(|row| *row < 250),
            "a rendered page came back blank"
        );
    }
}

fn refuses_other_files() {
    assert!(document::rasterise(b"\x89PNG\r\n\x1a\n").is_err());
    assert!(document::rasterise(b"").is_err());
}

fn broken_pdf() {
    let problem = document::rasterise(b"%PDF-1.4\nand then nothing")
        .expect_err("a truncated PDF should not render");
    assert!(
        problem.contains("pdftoppm") || problem.contains("no pages"),
        "unhelpful message: {problem}"
    );
}

fn leaves_no_litter() {
    let cache = gtk::glib::user_cache_dir().join("remarkable");
    let before = std::fs::read_dir(&cache).map(Iterator::count).unwrap_or(0);
    document::rasterise(&three_page_pdf()).expect("renders");
    let after = std::fs::read_dir(&cache).map(Iterator::count).unwrap_or(0);

    assert_eq!(before, after, "a render directory was left in {cache:?}");
}
