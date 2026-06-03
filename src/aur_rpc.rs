use crate::utils::run_command_with_output;
use crate::vlog;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct AurRpcResult {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Version")]
    version: String,
}

#[derive(Debug, Deserialize)]
struct AurRpcResponse {
    results: Vec<AurRpcResult>,
}

/// Basic URL encoder for package names
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    out
}

/// Fetch package details from AUR Web RPC API in batches
pub fn fetch_aur_packages_info(packages: &[String]) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    if packages.is_empty() {
        return Ok(map);
    }

    // Chunk the requests in batches of 40 to avoid URL length limits
    for chunk in packages.chunks(40) {
        let mut url = "https://aur.archlinux.org/rpc/?v=5&type=info".to_string();
        for pkg in chunk {
            url.push_str("&arg[]=");
            url.push_str(&url_encode(pkg));
        }

        vlog!("AUR RPC: Querying versions for chunk: {:?}", chunk);
        let start = std::time::Instant::now();
        let body = run_command_with_output(
            "curl",
            &[
                "-fsSL",
                "--compressed",
                "-m", "10", // 10 seconds timeout
                &url,
            ],
            None::<&str>,
        )?;
        vlog!("AUR RPC: Query returned in {:?}", start.elapsed());

        let response: AurRpcResponse = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse AUR RPC response JSON: {}", e))?;

        for res in response.results {
            map.insert(res.name, res.version);
        }
    }

    Ok(map)
}
