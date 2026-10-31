use reqwest;
use anyhow::Result;

pub struct SearchClient {
    client: reqwest::Client,
    url: String,
    api_key: String,
}

impl SearchClient {
    pub fn new(url: String, api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
            api_key,
        }
    }

    /// Tìm kiếm web qua API matilda.vn
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let response = self.client
            .get(&self.url)                          // GET https://api.matilda.vn?query=...
            .query(&[("query", query)])              // ?query=...
            .header("X-API-Key", &self.api_key)      // header X-API-Key
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Gọi search API lỗi: {}", e))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Đọc response lỗi: {}", e))?;

        if !status.is_success() {
            return Err(anyhow::anyhow!("Search API lỗi ({}): {}", status, body));
        }

        let data: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("Parse JSON lỗi: {} | raw: {}", e, body))?;

        // Format: { "results": [ { "title", "snippet", "url", "site_name" } ] }
        let results = data["results"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Response không có results: {}", body))?
            .iter()
            .take(limit)
            .map(|r| {
                let title = r["title"].as_str().unwrap_or("(không có tiêu đề)");
                let snippet = r["snippet"].as_str().unwrap_or("");
                let url = r["url"].as_str().unwrap_or("");
                format!("📌 {} — {} (nguồn: {})", title, snippet, url)
            })
            .collect();

        Ok(results)
    }
}
