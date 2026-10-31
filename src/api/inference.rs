#[allow(unused_imports)]
use reqwest;
use serde::Deserialize;


pub struct InferenceClient {
    client: reqwest::Client,
    url: String,
    api_key: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: String,
}

impl InferenceClient {
    pub fn new(url: String, api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
            api_key,
        }
    }

    pub async fn chat(&self, prompt: &str) -> Result<String, String> {
        let response = self.client
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({
                "messages": [
                    {
                        "role": "user",
                        "content": prompt
                    }
                ],
                "temperature": 0.7,
            }))
            .send()
            .await
            .map_err(|e| format!("Gọi API lỗi: {}", e))?;

        let chat_response = response
            .json::<ChatResponse>()
            .await
            .map_err(|e| format!("Parse response lỗi: {}", e))?;

        Ok(chat_response.choices.first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default())
    }
	    /// Gọi chat với tool calling (DeepSeek/OpenAI compatible)
    pub async fn chat_with_tools(
        &self,
        system: &str,
        user: &str,
        tools: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}/chat/completions", self.url.trim_end_matches('/'));

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": "deepseek-chat",
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user }
                ],
                "tools": tools,
                "tool_choice": "auto",
                "temperature": 0.3
            }))
            .send()
            .await
            .map_err(|e| format!("Gọi inference lỗi: {}", e))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(format!("Inference lỗi ({}): {}", status, body));
        }

        serde_json::from_str(&body)
            .map_err(|e| format!("Parse inference lỗi: {} | raw: {}", e, body))
    }
	    pub async fn chat_prompt(&self, system: &str, user: &str) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.url.trim_end_matches('/'));

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": "deepseek-chat",
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user }
                ],
                "temperature": 0.3
            }))
            .send()
            .await
            .map_err(|e| format!("Gọi inference lỗi: {}", e))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("Inference lỗi ({}): {}", status, body));
        }

        let data: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("Parse lỗi: {} | raw: {}", e, body))?;

        Ok(data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }
	    /// Gửi ảnh + câu hỏi cho model vision (chuẩn OpenAI-compatible)
    pub async fn chat_with_image(
        &self,
        model: &str,
        prompt: &str,
        image_base64: &str,
        mime: &str,
    ) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.url.trim_end_matches('/'));

        let body = serde_json::json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    { "type": "image_url", "image_url": {
                        "url": format!("data:{};base64,{}", mime, image_base64)
                    }}
                ]
            }],
            "temperature": 0.3
        });

        let response = self.client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Gọi inference lỗi: {}", e))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(format!("Inference lỗi ({}): {}", status, text));
        }

        let data: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("Parse lỗi: {} | raw: {}", e, text))?;

        Ok(data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

}
