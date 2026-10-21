use serde_json::Value;
use std::{env, fs, io::Read, sync::Arc, time::Duration};

pub(crate) fn download(url: &str, limit: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if !url.starts_with("https://") {
        return Err("Codefactor release URL must use HTTPS".into());
    }
    let mut builder = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .redirects(0);
    let ca = env::var("CODEFACTOR_CA_BUNDLE")
        .ok()
        .map(fs::read)
        .transpose()?;
    let local_ca = url.starts_with("https://gitlab.local/").then(|| {
        include_bytes!("../certs/gitlab.local.pem")
            .as_slice()
            .to_vec()
    });
    let mut tls = native_tls::TlsConnector::builder();
    if let Some(ca) = ca.or(local_ca) {
        tls.add_root_certificate(native_tls::Certificate::from_pem(&ca)?);
    }
    builder = builder.tls_connector(Arc::new(tls.build()?));
    let agent = builder.build();
    let mut url = url.to_owned();
    for redirects in 0..=5 {
        if !url.starts_with("https://") {
            return Err("Codefactor release URL must use HTTPS".into());
        }
        let mut request = agent.get(&url);
        let mut authenticated = false;
        if redirects == 0 && !url.starts_with("https://github.com/") {
            if let Ok(token) = env::var("CODEFACTOR_TOKEN") {
                request = request.set("PRIVATE-TOKEN", &token);
                authenticated = true;
            } else if let Ok(token) =
                env::var("CODEFACTOR_JOB_TOKEN").or_else(|_| env::var("CI_JOB_TOKEN"))
            {
                request = request.set("JOB-TOKEN", &token);
                authenticated = true;
            }
        }
        let response = request.call()?;
        if matches!(response.status(), 301 | 302 | 303 | 307 | 308) {
            if authenticated || redirects == 5 {
                return Err("Unsafe Codefactor release redirect".into());
            }
            url = response
                .header("Location")
                .ok_or("Missing redirect location")?
                .to_owned();
            continue;
        }
        if response.status() != 200 {
            return Err("Codefactor download failed".into());
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err("Codefactor download exceeded size limit".into());
        }
        return Ok(bytes);
    }
    Err("Too many Codefactor release redirects".into())
}

pub(crate) fn release_base(
    config: &Value,
    version: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let supplied = env::var("CODEFACTOR_RELEASE_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty());
    if version == "latest" && supplied.is_none() {
        return Ok(config["latest_url"]
            .as_str()
            .ok_or("Missing latest release URL")?
            .trim_end_matches('/')
            .into());
    }
    let base = supplied.unwrap_or(
        config["base_url"]
            .as_str()
            .ok_or("Missing release URL")?
            .into(),
    );
    Ok(if base.contains("{version}") {
        base.trim_end_matches('/').replace("{version}", version)
    } else {
        format!("{}/{version}", base.trim_end_matches('/'))
    })
}

pub(crate) fn valid_version(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}
