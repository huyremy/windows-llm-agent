use crate::api::{inference::InferenceClient, search::SearchClient};
use crate::db::vector::VectorStore;
use anyhow::Result;
use std::sync::Arc;

pub struct Agent {
    inference: Arc<InferenceClient>,
    search: Arc<SearchClient>,
    vector_store: Arc<VectorStore>,
}

/// Marker để LLM yêu cầu search web
const SEARCH_MARKER: &str = "SEARCH:";

impl Agent {
    pub fn new(
        inference: Arc<InferenceClient>,
        search: Arc<SearchClient>,
        vector_store: Arc<VectorStore>,
    ) -> Self {
        Self { inference, search, vector_store }
    }

        pub async fn process(&self, query: &str) -> Result<String> {
        // 1. Qdrant search + danh sách file
        let docs = self.vector_store.search(query, 5).await.unwrap_or_default();
        let all_files = self.vector_store.list_files().await.unwrap_or_default();

        // 2. System prompt
        let system = "Bạn là trợ lý AI. Khi cần thông tin thời gian thực (thời tiết, tin tức, giá cả...), \
                      HÃY GỌI tool 'web_search'. Khi trả lời câu hỏi thông thường, trả lời trực tiếp.";

        // 3. User message = context + câu hỏi
        let mut user = String::new();
        if !docs.is_empty() {
            user.push_str("TÀI LIỆU LIÊN QUAN:\n");
            for d in &docs {
                let t = d.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let f = d.get("filename").and_then(|v| v.as_str()).unwrap_or("");
                user.push_str(&format!("({}) {}\n", f, t));
            }
            user.push('\n');
        }
        if !all_files.is_empty() {
            user.push_str(&format!("Tài liệu đã nạp: {}\n\n", all_files.join(", ")));
        }
        user.push_str(&format!("Câu hỏi: {}", query));

        // 4. Tools definition
        let tools = serde_json::json!([{
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Tìm kiếm thông tin trên web. Dùng khi cần dữ liệu thời gian thực: \
                                thời tiết, tin tức, giá cả, sự kiện gần đây.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Từ khóa tìm kiếm"
                        }
                    },
                    "required": ["query"]
                }
            }
        }]);

        // 5. Gọi DeepSeek với tools
        let response = self
            .inference
            .chat_with_tools(system, &user, tools)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        let message = &response["choices"][0]["message"];

        // 6. Kiểm tra có tool_calls không
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            if let Some(first_call) = tool_calls.first() {
                let func_name = first_call["function"]["name"].as_str().unwrap_or("");
                if func_name == "web_search" {
                    let args_str = first_call["function"]["arguments"]
                        .as_str()
                        .unwrap_or("{}");
                    let args: serde_json::Value = serde_json::from_str(args_str)
                        .unwrap_or(serde_json::json!({}));
                    let wq = args["query"].as_str().unwrap_or(query);

                    println!("🌐 Đang tìm trên web: {}...", wq);

                    let web_results = self.search.search(wq, 5).await.unwrap_or_default();

                    // 7. Gọi lại DeepSeek với kết quả web
                    let mut final_user = user.clone();
                    final_user.push_str("\n\nKẾT QUẢ TÌM KIẾM WEB:\n");
                    for r in &web_results {
                        final_user.push_str(&format!("- {}\n", r));
                    }
                    final_user.push_str("\nHãy trả lời câu hỏi dựa trên kết quả trên.");

                    let final_response = self
                        .inference
                        .chat_prompt(system, &final_user)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?;

                    return Ok(final_response);
                }
            }
        }

        // 8. Không có tool_calls → trả lời thẳng
        let content = message["content"].as_str().unwrap_or("(không có nội dung)");
        Ok(content.to_string())
    }


    /// Trích "SEARCH: xxx" từ response của LLM (nếu có)
    fn extract_search_query(response: &str) -> Option<String> {
        let trimmed = response.trim();
        if trimmed.starts_with(SEARCH_MARKER) {
            let q = trimmed.trim_start_matches(SEARCH_MARKER).trim().to_string();
            if !q.is_empty() {
                return Some(q);
            }
        }
        None
    }

    fn build_prompt(
        &self,
        query: &str,
        docs: &[serde_json::Value],
        all_files: &[String],
        web_results: &[String],
    ) -> String {
        let mut parts = Vec::new();

        parts.push(
            "Bạn là trợ lý AI hữu ích trên Windows. Nguyên tắc:\n\
             1. Ưu tiên TÀI LIỆU và KẾT QUẢ WEB bên dưới làm nguồn chính.\n\
             2. Nếu câu hỏi về kiến thức chung KHÔNG cần tài liệu/web → trả lời bằng kiến thức của bạn.\n\
             3. Nếu cần thông tin THỜI GIAN THỰC (tin tức, giá, thời tiết, sự kiện gần đây) mà chưa có dữ liệu → \
               trả lời ĐÚNG 1 DÒNG: SEARCH: <từ khóa>\n\
             4. Nếu người dùng muốn tóm tắt/xem nội dung tài liệu đã nạp nhưng đoạn tìm được không đủ → \
               nói rõ có những file nào (dựa danh sách bên dưới) và đề nghị hỏi cụ thể hơn.\n\
             5. Không bịa đặt. Trả lời bằng tiếng Việt."
                .to_string(),
        );

        if !all_files.is_empty() {
            parts.push(format!(
                "=== TÀI LIỆU ĐÃ NẠP ({} file) ===\n{}",
                all_files.len(),
                all_files.join(", ")
            ));
        } else {
            parts.push("=== TÀI LIỆU ĐÃ NẠP ===\n(chưa có tài liệu nào)".to_string());
        }

        if !docs.is_empty() {
            parts.push("=== ĐOẠN TÀI LIỆU LIÊN QUAN ===".to_string());
            for (i, doc) in docs.iter().enumerate() {
                let text = doc.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let filename = doc.get("filename").and_then(|v| v.as_str()).unwrap_or("?");
                parts.push(format!("[{}] ({}): {}", i + 1, filename, text));
            }
        }

        if !web_results.is_empty() {
            parts.push("=== KẾT QUẢ WEB ===".to_string());
            for r in web_results {
                parts.push(format!("- {}", r));
            }
        }

        parts.push(format!("Câu hỏi: {}", query));
        parts.join("\n\n")
    }
}
