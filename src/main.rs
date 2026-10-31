use anyhow::Result;
use std::io::Write;
use std::sync::Arc;
use std::env;

use windows_llm_agent::agent::{Agent, Memory};
use windows_llm_agent::api::{inference::InferenceClient, search::SearchClient};
use windows_llm_agent::db::vector::VectorStore;


#[tokio::main]
async fn main() -> Result<()> {
    // Load biến môi trường từ .env
    dotenv::dotenv().ok();

    // Khởi tạo logging
    tracing_subscriber::fmt::init();
    tracing::info!("Windows LLM Agent đang khởi động...");

    // Kiểm tra Ollama có chạy chưa
    if reqwest::get("http://localhost:11434/api/tags").await.is_err() {
        println!("⚠️ Ollama chưa chạy! Hãy mở terminal khác chạy: ollama serve");
    }

    // 1. Đọc config
    let inference = Arc::new(InferenceClient::new(
        env::var("INFERENCE_URL").expect("INFERENCE_URL must be set"),
        env::var("INFERENCE_API_KEY").expect("INFERENCE_API_KEY must be set"),
    ));

    let search = Arc::new(SearchClient::new(
        env::var("SEARCH_URL").expect("SEARCH_URL must be set"),
        env::var("SEARCH_API_KEY").expect("SEARCH_API_KEY must be set"),
    ));

    let vector_store = Arc::new(VectorStore::new(
        &env::var("QDRANT_URL").expect("QDRANT_URL must be set"),
        &env::var("QDRANT_API_KEY").expect("QDRANT_API_KEY must be set"),
        "documents",
        "nomic-embed-text",
    ).map_err(|e| anyhow::anyhow!(e))?);


    // Init collection (tạo nếu chưa có)
    vector_store.init_collection().await
        .map_err(|e| anyhow::anyhow!(e))?;


    // 2. Khởi tạo Agent + Memory
    let agent = Agent::new(inference.clone(), search, vector_store.clone());
    let mut memory = Memory::new(10);

    // Model vision (đặt trong .env, vd VISION_MODEL=deepseek-vl2)
    let vision_model = env::var("VISION_MODEL").unwrap_or_else(|_| "deepseek-chat".to_string());

    // 3. Chat loop
    println!("╔════════════════════════════════════════════╗");
    println!("║    Matilda LLM Version v0.1.0             ║");
    println!("║    Type 'exit' to quit                    ║");
    println!("║    /add <file>      - nạp txt/pdf/docx    ║");
    println!("║    /addurl <url>    - nạp trang web       ║");
    println!("║    /img <ảnh> | <câu hỏi> - hỏi về ảnh    ║");
    println!("║    /list            - xem tài liệu đã nạp ║");
    println!("║    /remove <file>   - gỡ tài liệu         ║");
    println!("╚════════════════════════════════════════════╝");

    loop {
        let mut input = String::new();
        print!("👤 Bạn: ");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_line(&mut input).unwrap();

        let input = input.trim();
        if input == "exit" || input.is_empty() {
            break;
        }

        // ===== LỆNH /add <file> =====
        if let Some(path) = input.strip_prefix("/add ") {
            let path = path.trim();
            match ingest_file(&vector_store, path).await {
                Ok(n) => println!("✅ Đã nạp {} đoạn từ '{}'", n, path),
                Err(e) => println!("❌ Lỗi nạp file: {}", e),
            }
            println!();
            continue;
        }

        // ===== LỆNH /addurl <url> =====
        if let Some(url) = input.strip_prefix("/addurl ") {
            let url = url.trim();
            if url.is_empty() || !url.starts_with("http") {
                println!("⚠️ Dùng: /addurl https://example.com/trang");
            } else {
                match add_url(&vector_store, url).await {
                    Ok(n) => println!("✅ Đã nạp {} đoạn từ '{}'", n, url),
                    Err(e) => println!("❌ Lỗi nạp URL: {}", e),
                }
            }
            println!();
            continue;
        }

        // ===== LỆNH /img <đường_dẫn_ảnh> | <câu hỏi> =====
        if let Some(rest) = input.strip_prefix("/img ") {
            let (img_path, question) = match rest.split_once('|') {
                Some((p, q)) => (p.trim(), q.trim()),
                None => (rest.trim(), "Hãy mô tả chi tiết nội dung ảnh này."),
            };

            let img_path = normalize_path(img_path);
            if !std::path::Path::new(&img_path).exists() {
                println!("❌ Không tìm thấy ảnh: '{}'", img_path);
                println!();
                continue;
            }

            match windows_llm_agent::util::extract::image_to_base64(&img_path) {
                Ok((b64, mime)) => {
                    println!("🖼️ Đang gửi ảnh cho model vision...");
                    match inference
                        .chat_with_image(&vision_model, question, &b64, &mime)
                        .await
                    {
                        Ok(answer) => println!("🤖 Agent: {}", answer),
                        Err(e) => println!("❌ Lỗi: {}", e),
                    }
                }
                Err(e) => println!("❌ Lỗi: {}", e),
            }
            println!();
            continue;
        }

        // ===== LỆNH /list =====
        if input == "/list" {
            match vector_store.list_files().await {
                Ok(files) => {
                    if files.is_empty() {
                        println!("📂 Chưa có tài liệu nào được nạp.");
                    } else {
                        println!("📂 Có {} tài liệu:", files.len());
                        for (i, f) in files.iter().enumerate() {
                            println!("   {}. {}", i + 1, f);
                        }
                    }
                }
                Err(e) => println!("❌ Lỗi: {}", e),
            }
            println!();
            continue;
        }

        // ===== LỆNH /remove <file> =====
        if let Some(fname) = input.strip_prefix("/remove ") {
            let fname = fname.trim();
            if fname.is_empty() {
                println!("⚠️ Dùng: /remove <tên_file>");
            } else {
                match vector_store.delete_file(fname).await {
                    Ok(0) => println!("⚠️ Không tìm thấy file '{}' nào", fname),
                    Ok(n) => println!("🗑️ Đã xóa {} đoạn của '{}'", n, fname),
                    Err(e) => println!("❌ Lỗi xóa: {}", e),
                }
            }
            println!();
            continue;
        }

        // Gán role vào memory
        memory.append("user", input);

        // Xử lý bởi agent
        match agent.process(input).await {
            Ok(response) => {
                println!("🤖 Agent: {}", response);
                memory.append("assistant", &response);
            }
            Err(e) => {
                println!("❌ Lỗi: {}", e);
            }
        }
        println!();
    }

    tracing::info!("Tạm biệt!");
    Ok(())
}

/// Chuẩn hóa đường dẫn: hỗ trợ "C:\...", "C:/...", "file:///C:/..."
fn normalize_path(raw: &str) -> String {
    let mut p = raw.trim().to_string();

    // Bỏ prefix file:// hoặc file:///
    if let Some(rest) = p.strip_prefix("file://") {
        p = rest.to_string();
    }
    // Bỏ "/" đầu: "/C:/Users/..." → "C:/Users/..."
    p = p.trim_start_matches('/').to_string();

    // Trên Windows đổi "/" → "\"
    #[cfg(windows)]
    {
        p = p.replace('/', "\\");
    }

    // Bỏ dấu nháy kép nếu user dán kèm
    p = p.trim_matches('"').to_string();

    p
}

/// Đọc file (txt/pdf/docx/doc), trích text rồi nạp vào Qdrant
async fn ingest_file(store: &VectorStore, path: &str) -> Result<usize, String> {
    // Chuẩn hóa path (hỗ trợ file:///C:/...)
    let path = normalize_path(path);

    if !std::path::Path::new(&path).exists() {
        return Err(format!("Không tìm thấy file: '{}'", path));
    }

    println!("📄 Đang trích nội dung '{}'...", path);
    let text = windows_llm_agent::util::extract::extract_from_file(&path)
        .map_err(|e| format!("{}", e))?;

    if text.trim().is_empty() {
        return Err("Không trích được nội dung nào (file rỗng hoặc PDF scan ảnh?)".to_string());
    }

    let filename = std::path::Path::new(&path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    ingest_text(store, &filename, &text).await
}

/// Tải 1 URL, scrape text rồi nạp vào Qdrant
async fn add_url(store: &VectorStore, url: &str) -> Result<usize, String> {
    println!("🌐 Đang tải trang '{}'...", url);
    let text = windows_llm_agent::util::extract::scrape_url(url)
        .await
        .map_err(|e| format!("{}", e))?;

    if text.trim().is_empty() {
        return Err("Trang không có text (có thể là trang render bằng JS)".to_string());
    }

    let filename = windows_llm_agent::util::extract::url_to_filename(url);
    println!("📌 Lưu với tên: {}", filename);

    ingest_text(store, &filename, &text).await
}

/// Tách đoạn ~500 ký tự + embed + lưu (dùng chung cho file và URL)
async fn ingest_text(store: &VectorStore, filename: &str, text: &str) -> Result<usize, String> {
    let chars: Vec<char> = text.chars().collect();
    let chunks: Vec<String> = chars
        .chunks(500)
        .map(|c| c.iter().collect::<String>())
        .filter(|s| !s.trim().is_empty())
        .collect();

    let total = chunks.len();
    for (i, chunk) in chunks.iter().enumerate() {
        let doc_id = uuid::Uuid::new_v4().to_string();
        store.add_file(&doc_id, filename, chunk).await
            .map_err(|e| format!("Lỗi chunk {}: {}", i, e))?;
        println!("   📄 Đã nạp đoạn {}/{}", i + 1, total);
    }

    Ok(total)
}
