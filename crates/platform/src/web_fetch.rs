//! Browser HTTP capability used by Application-owned asynchronous tasks.

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// Fetch a successful HTTP response as UTF-8 text.
///
/// Keeping the browser API here prevents the Application layer from depending
/// on `web-sys` types while still allowing it to own the task lifecycle and
/// parse the response into its platform-neutral DTOs.
pub async fn fetch_text(url: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "浏览器窗口不可用".to_owned())?;
    let response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|error| format!("请求失败：{error:?}"))?
        .dyn_into::<web_sys::Response>()
        .map_err(|error| format!("浏览器返回了无效响应：{error:?}"))?;
    if !response.ok() {
        return Err(format!("请求失败：HTTP {} ({url})", response.status()));
    }
    JsFuture::from(
        response
            .text()
            .map_err(|error| format!("读取响应失败：{error:?}"))?,
    )
    .await
    .map_err(|error| format!("读取响应失败：{error:?}"))?
    .as_string()
    .ok_or_else(|| "响应不是有效文本".to_owned())
}
