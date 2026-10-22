//! Bounded HTTP body reads.

/// Reads a response body, failing once it grows past `limit` bytes. The body is never held
/// beyond `limit + one chunk`, whatever `Content-Length` claimed.
pub(crate) async fn read_capped(
    mut resp: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    if resp.content_length().is_some_and(|n| n > limit as u64) {
        return Err("response exceeds size limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| e.without_url().to_string())?
    {
        if body.len() + chunk.len() > limit {
            return Err("response exceeds size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
