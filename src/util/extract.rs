use anyhow::{Result, anyhow};

/// Trích text từ file theo đuôi: .txt/.md, .pdf, .docx
pub fn extract_from_file(path: &str) -> Result<String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "txt" | "md" | "json" | "csv" | "log" | "rs" | "py" | "js" => {
            Ok(std::fs::read_to_string(path)?)
        }
        "pdf" => extract_pdf(path),
        "docx" => extract_docx(path),
        "doc" => extract_doc(path),
        other => Err(anyhow!("Chưa hỗ trợ định dạng .{} (chỉ .txt/.md/.pdf/.docx)", other)),
    }
}

/// Đọc PDF
fn extract_pdf(path: &str) -> Result<String> {
    let text = pdf_extract::extract_text(path)
        .map_err(|e| anyhow!("Đọc PDF lỗi: {}", e))?;
    Ok(text)
}

/// Đọc DOCX: giải nén word/document.xml rồi strip tag XML
fn extract_docx(path: &str) -> Result<String> {
    use std::io::Read;

    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let mut xml = String::new();
    let mut entry = archive
        .by_name("word/document.xml")
        .map_err(|_| anyhow!("File .docx không hợp lệ (thiếu word/document.xml)"))?;
    entry.read_to_string(&mut xml)?;

    // Mỗi đoạn văn </w:p> → xuống dòng, rồi bỏ hết tag <...>
    let xml = xml.replace("</w:p>", "\n");
    let mut out = String::new();
    let mut in_tag = false;
    for c in xml.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }

    Ok(out)
}
/// Đọc .doc (Word cũ): dùng Word COM qua PowerShell convert sang .docx tạm rồi đọc
fn extract_doc(path: &str) -> Result<String> {
    let tmp_docx = std::env::temp_dir()
        .join(format!("agent_convert_{}.docx", std::process::id()));
    let _ = std::fs::remove_file(&tmp_docx); // dọn file tạm cũ nếu còn

    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$word = New-Object -ComObject Word.Application
$word.Visible = $false
try {{
    $doc = $word.Documents.Open('{}', $false, $true)
    $doc.SaveAs2('{}', 16)
    $doc.Close($false)
}} finally {{
    $word.Quit()
}}
"#,
        path.replace('\'', "''"),                              // escape nháy đơn
        tmp_docx.to_string_lossy().replace('\'', "''")
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|e| anyhow!("Không chạy được PowerShell: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "Chuyển .doc lỗi (máy cần cài MS Word): {}\n\
             👉 Cách nhanh: mở file .doc bằng Word → Save As → .docx → /add lại.",
            stderr.trim()
        ));
    }

    let text = extract_docx(tmp_docx.to_string_lossy().as_ref())?;
    let _ = std::fs::remove_file(&tmp_docx); // dọn dẹp
    Ok(text)
}
/// Đọc file ảnh → (base64, mime type)
pub fn image_to_base64(path: &str) -> Result<(String, String)> {
    use base64::Engine;

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        other => {
            return Err(anyhow!(
                "Định dạng ảnh .{} chưa hỗ trợ (png/jpg/jpeg/webp/gif/bmp)",
                other
            ))
        }
    };

    let bytes = std::fs::read(path)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((b64, mime.to_string()))
}

/// Scrape 1 URL → text thuần (html2text tự bỏ script/style/tag)
/// Scrape 1 URL → text thuần. Tự nhận PDF và trích text luôn.
pub async fn scrape_url(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let resp = client
        .get(url)
        .send()
        .await?
        .error_for_status()
        .map_err(|e| anyhow!("Trang trả lỗi: {} (URL đúng chưa?)", e))?;

    // Kiểm tra content-type TRƯỚC khi đọc nội dung
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let bytes = resp.bytes().await?;

    // 📄 Là PDF (theo header hoặc magic bytes) → trích text trực tiếp
    if content_type.contains("pdf") || bytes.starts_with(b"%PDF") {
        let text = pdf_extract::extract_text_from_mem(&bytes)
            .map_err(|e| anyhow!("Đọc PDF lỗi: {}", e))?;
        return Ok(text);
    }

    // 🚫 Chặn binary lạ (ảnh, zip, video...)
    if !content_type.contains("text/html")
        && !content_type.contains("text/plain")
        && !content_type.is_empty()
    {
        return Err(anyhow!(
            "URL trả về '{}' — không phải trang web hay PDF, bỏ qua.",
            content_type
        ));
    }

    // 🌐 Trang HTML thường
    let html = String::from_utf8_lossy(&bytes);
    let text = html2text::from_read(html.as_bytes(), 200);

    let cleaned: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(cleaned.join("\n"))
}


/// Đổi URL thành tên "file" để lưu trong Qdrant
/// https://matilda.vn/blog/ai → web_matilda.vn_blog_ai
pub fn url_to_filename(url: &str) -> String {
    let clean = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(['?', '#'])
        .next()
        .unwrap_or(url);
    let clean: String = clean
        .trim_end_matches('/')
        .chars()
        .map(|c| if c == '/' { '_' } else { c })
        .collect();
    format!("web_{}", clean)
}
