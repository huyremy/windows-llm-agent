use reqwest;
use serde_json::Value;
use anyhow::Result;

pub struct VectorStore {
    client: reqwest::Client,
    url: String,
    api_key: String,
    collection_name: String,
    embed_model: String,
}

impl VectorStore {
    pub fn new(url: &str, api_key: &str, collection_name: &str, embed_model: &str) -> Result<Self, String> {
        let url = url.trim_end_matches('/').to_string();

        Ok(Self {
            client: reqwest::Client::new(),
            url,
            api_key: api_key.to_string(),
            collection_name: collection_name.to_string(),
            embed_model: embed_model.to_string(),
        })
    }

    /// Tạo collection (chỉ cần chạy 1 lần)
    pub async fn init_collection(&self) -> Result<(), String> {
        let url = format!("{}/collections/{}", self.url, self.collection_name);

        let response = self.client
            .put(&url)
            .header("api-key", &self.api_key)
            .json(&serde_json::json!({
                "vectors": {
                    "size": 768,   // nomic-embed-text = 768 dims
                    "distance": "Cosine"
                }
            }))
            .send()
            .await
            .map_err(|e| format!("Kết nối Qdrant lỗi: {}", e))?;

        if response.status().is_success() {
            tracing::info!("Tạo collection '{}'", self.collection_name);
        } else {
            // Collection đã tồn tại → bỏ qua
            if !response.status().as_u16().to_string().contains("409") {
                let error = response.text().await.unwrap_or_default();
                return Err(format!("Tạo collection lỗi: {}", error));
            }
        }
        self.ensure_filename_index().await?;

        Ok(())
    }

    /// Embed text thành vector (gọi Ollama local)
    pub async fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let model = std::env::var("OLLAMA_EMBED_MODEL")
            .unwrap_or_else(|_| "nomic-embed-text".to_string());
        let base_url = std::env::var("OLLAMA_URL")
            .unwrap_or_else(|_| "http://localhost:11434".to_string());

        let url = format!("{}/api/embed", base_url);

        let response = self.client
            .post(&url)
            .json(&serde_json::json!({
                "model": model,
                "input": texts,
            }))
            .send()
            .await
            .map_err(|e| format!("Gọi Ollama lỗi (đã chạy 'ollama serve' chưa?): {}", e))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| format!("Đọc response lỗi: {}", e))?;

        if !status.is_success() {
            return Err(format!("Ollama lỗi ({}): {}", status, body));
        }

        let data: Value = serde_json::from_str(&body)
            .map_err(|e| format!("Parse Ollama JSON lỗi: {} | raw: {}", e, body))?;

        let embeddings: Vec<Vec<f32>> = data["embeddings"]
            .as_array()
            .ok_or_else(|| format!("Response không có embeddings: {}", body))?
            .iter()
            .map(|v| {
                v.as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                    .collect()
            })
            .collect();

        if embeddings.is_empty() {
            return Err(format!("Không lấy được vector nào: {}", body));
        }

        Ok(embeddings)
    }

    /// Lưu 1 tài liệu (text + vector + metadata)
    pub async fn add_document(
        &self,
        vector: &[f32],
        doc_id: &str,
        payload: Value,
    ) -> Result<(), String> {
        let point = serde_json::json!({
            "id": doc_id,
            "vector": vector,
            "payload": payload,
        });

        let url = format!(
            "{}/collections/{}/points?wait=true",
            self.url, self.collection_name
        );

        let response = self.client
            .put(&url)
            .header("api-key", &self.api_key)
            .json(&serde_json::json!({
                "points": [point]
            }))
            .send()
            .await
            .map_err(|e| format!("Gọi Qdrant API lỗi: {}", e))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(format!("Lưu vector lỗi ({}): {}", status, body));
        }

        Ok(())
    }

    /// Thêm 1 file tài liệu (đoạn + metadata)
    pub async fn add_file(
        &self,
        doc_id: &str,
        filename: &str,
        text: &str,
    ) -> Result<(), String> {
        let vector = self.embed_texts(&[text.to_string()])
            .await?
            .pop()
            .ok_or_else(|| "Không tạo được vector".to_string())?;

        let payload = serde_json::json!({
            "text": text,
            "filename": filename,
            "created_at": chrono::Utc::now().to_rfc3339(),
        });

        self.add_document(&vector, doc_id, payload).await
    }

    /// Tìm kiếm tài liệu liên quan
    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<Value>, String> {
        let query_vector = self.embed_texts(&[query.to_string()])
            .await?
            .pop()
            .ok_or_else(|| "Không embed được query".to_string())?;

        let url = format!(
            "{}/collections/{}/points/query",
            self.url, self.collection_name
        );

        let response = self.client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&serde_json::json!({
                "query": query_vector,
                "limit": top_k,
                "with_payload": true,
            }))
            .send()
            .await
            .map_err(|e| format!("Gọi Qdrant API lỗi: {}", e))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(format!("Search lỗi ({}): {}", status, body));
        }

        let data: Value = serde_json::from_str(&body)
            .map_err(|e| format!("Parse search JSON lỗi: {} | raw: {}", e, body))?;

        // Qdrant query mới trả về "result"."points"
        let points = data["result"]["points"]
            .as_array()
            .or_else(|| data["result"].as_array())
            .ok_or_else(|| format!("Response không có kết quả: {}", body))?;

        let results: Vec<Value> = points
            .iter()
            .map(|r| r["payload"].clone())
            .collect();

        Ok(results)
    }

    /// Liệt kê danh sách các file đã nạp (lấy filename duy nhất)
    pub async fn list_files(&self) -> Result<Vec<String>, String> {
        let url = format!(
            "{}/collections/{}/points/scroll",
            self.url, self.collection_name
        );

        let mut all_files: Vec<String> = Vec::new();
        let mut offset: Option<Value> = None;

        loop {
            let mut body = serde_json::json!({
                "limit": 256,
                "with_payload": true,
            });
            if let Some(off) = &offset {
                body["offset"] = off.clone();
            }

            let response = self.client
                .post(&url)
                .header("api-key", &self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("Scroll lỗi: {}", e))?;

            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(format!("Scroll lỗi ({}): {}", status, text));
            }

            let data: Value = serde_json::from_str(&text)
                .map_err(|e| format!("Parse scroll lỗi: {} | raw: {}", e, text))?;

            let points = data["result"]["points"]
                .as_array()
                .ok_or_else(|| format!("Response không có points: {}", text))?;

            for p in points {
                if let Some(fname) = p["payload"]["filename"].as_str() {
                    if !all_files.contains(&fname.to_string()) {
                        all_files.push(fname.to_string());
                    }
                }
            }

            // Lấy offset cho trang tiếp theo (null = hết)
            offset = data["result"]["next_page_offset"]
                .as_object()
                .map(|_| data["result"]["next_page_offset"].clone());
            if offset.is_none() {
                break;
            }
        }

        all_files.sort();
        Ok(all_files)
    }

    /// Xóa tất cả điểm của 1 file theo filename
    pub async fn delete_file(&self, filename: &str) -> Result<u64, String> {
        // Đếm trước để báo user
        let count = self.count_points_of_file(filename).await?;

        let url = format!(
            "{}/collections/{}/points/delete?wait=true",
            self.url, self.collection_name
        );

        let response = self.client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&serde_json::json!({
                "filter": {
                    "must": [
                        { "key": "filename", "match": { "value": filename } }
                    ]
                }
            }))
            .send()
            .await
            .map_err(|e| format!("Gọi Qdrant API lỗi: {}", e))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("Xóa lỗi ({}): {}", status, body));
        }

        Ok(count)
    }

    /// Đếm số đoạn của 1 file
    async fn count_points_of_file(&self, filename: &str) -> Result<u64, String> {
        let url = format!(
            "{}/collections/{}/points/count",
            self.url, self.collection_name
        );

        let response = self.client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&serde_json::json!({
                "filter": {
                    "must": [
                        { "key": "filename", "match": { "value": filename } }
                    ]
                },
                "exact": true
            }))
            .send()
            .await
            .map_err(|e| format!("Gọi Qdrant API lỗi: {}", e))?;

        let body = response.text().await.unwrap_or_default();
        let data: Value = serde_json::from_str(&body)
            .map_err(|e| format!("Parse count lỗi: {} | raw: {}", e, body))?;

        Ok(data["result"]["count"].as_u64().unwrap_or(0))
    }
	 pub async fn ensure_filename_index(&self) -> Result<(), String> {
	        let url = format!("{}/collections/{}/index", self.url, self.collection_name);

	        let response = self.client
	            .put(&url)
	            .header("api-key", &self.api_key)
	            .json(&serde_json::json!({
	                "field_name": "filename",
	                "field_schema": "keyword"
	            }))
	            .send()
	            .await
	            .map_err(|e| format!("Tạo index lỗi: {}", e))?;

	        let status = response.status();
	        let body = response.text().await.unwrap_or_default();

	        // 200 = tạo mới hoặc đã tồn tại (idempotent)
	        if !status.is_success() {
	            // Bỏ qua nếu index đã tồn tại
	            if !body.to_lowercase().contains("already exists") {
	                return Err(format!("Tạo index lỗi ({}): {}", status, body));
	            }
	        }

	        tracing::info!("Index 'filename' đã sẵn sàng");
	        Ok(())
	    }
}
