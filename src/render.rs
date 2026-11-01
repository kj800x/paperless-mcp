//! Renders PDF pages to JPEG with poppler's `pdftoppm`.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

const RENDER_TIMEOUT: Duration = Duration::from_secs(90);
const JPEG_QUALITY: u32 = 85;

/// Renders pages `first..=last` (1-based) of `pdf` at `dpi`. Returns one JPEG per
/// page, in page order. Pages past the end of the document are simply absent.
pub async fn render_pdf_pages(
    pdf: &[u8],
    first: u32,
    last: u32,
    dpi: u32,
) -> Result<Vec<Vec<u8>>, String> {
    let dir = tempfile::tempdir().map_err(|e| format!("Couldn't create temp dir: {}", e))?;
    let input = dir.path().join("in.pdf");
    tokio::fs::write(&input, pdf)
        .await
        .map_err(|e| format!("Couldn't write temp file: {}", e))?;
    let prefix = dir.path().join("page");

    let mut command = Command::new("pdftoppm");
    command
        .arg("-jpeg")
        .arg("-jpegopt")
        .arg(format!("quality={}", JPEG_QUALITY))
        .arg("-r")
        .arg(dpi.to_string())
        .arg("-f")
        .arg(first.to_string())
        .arg("-l")
        .arg(last.to_string())
        .arg(&input)
        .arg(&prefix)
        .kill_on_drop(true);

    let output = tokio::time::timeout(RENDER_TIMEOUT, command.output())
        .await
        .map_err(|_| "Rendering the PDF timed out".to_string())?
        .map_err(|e| format!("Couldn't run pdftoppm: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("pdftoppm failed: {}", stderr.trim()));
    }

    read_pages(dir.path()).await
}

/// pdftoppm names files `page-<n>.jpg`, zero-padding `n` to the document's page
/// count width, so sort numerically rather than by name.
async fn read_pages(dir: &Path) -> Result<Vec<Vec<u8>>, String> {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("Couldn't read rendered pages: {}", e))?;
    let mut pages: Vec<(u32, std::path::PathBuf)> = vec![];
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Couldn't read rendered pages: {}", e))?
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(n) = page_number(&name) {
            pages.push((n, entry.path()));
        }
    }
    pages.sort_by_key(|(n, _)| *n);

    let mut out = Vec::with_capacity(pages.len());
    for (_, path) in pages {
        out.push(
            tokio::fs::read(&path)
                .await
                .map_err(|e| format!("Couldn't read rendered page: {}", e))?,
        );
    }
    Ok(out)
}

fn page_number(file_name: &str) -> Option<u32> {
    file_name
        .strip_prefix("page-")?
        .strip_suffix(".jpg")?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_file_names() {
        assert_eq!(page_number("page-1.jpg"), Some(1));
        assert_eq!(page_number("page-012.jpg"), Some(12));
        assert_eq!(page_number("in.pdf"), None);
        assert_eq!(page_number("page-x.jpg"), None);
    }

    /// Two blank pages; xref offsets are omitted, which poppler tolerates by reconstructing.
    const TWO_PAGE_PDF: &[u8] = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF
";

    #[tokio::test]
    async fn renders_requested_pages() {
        if std::process::Command::new("pdftoppm")
            .arg("-v")
            .output()
            .is_err()
        {
            eprintln!("pdftoppm not installed; skipping");
            return;
        }
        let pages = render_pdf_pages(TWO_PAGE_PDF, 1, 5, 72).await.unwrap();
        assert_eq!(pages.len(), 2);
        assert!(
            pages.iter().all(|p| p.starts_with(&[0xFF, 0xD8])),
            "not JPEG"
        );

        let second = render_pdf_pages(TWO_PAGE_PDF, 2, 2, 72).await.unwrap();
        assert_eq!(second.len(), 1);
    }
}
