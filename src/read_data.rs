use std::{error::Error, fs};

pub fn load_csv_amplitude_series(
    path: &str,
    subcarrier: usize,
) -> Result<Vec<(f64, f64)>, Box<dyn Error + Send + Sync>> {
    let content = fs::read_to_string(path)?;
    let mut lines = content.lines();
    let _header = lines.next().ok_or("CSV file is empty")?;
    let i_col = 2 + 2 * subcarrier;
    let q_col = 3 + 2 * subcarrier;
    let mut first_ts: Option<u64> = None;
    let mut out = Vec::new();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() <=  q_col {
            continue;
        }
        let ts: u64 = match parts[0].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let i: f64 = match parts[i_col].parse::<i32>() {
            Ok(v) => v as f64,
            Err(_) => continue,
        };
        let q: f64 = match parts[q_col].parse::<i32>() {
            Ok(v) => v as f64,
            Err(_) => continue,
        };
        let amp: f64 = (i * i + q * q).sqrt();
        let t: f64 = if let Some(ts0) = first_ts {
            (ts - ts0) as f64 / 1e6
        } else {
            first_ts = Some(ts);
            0.0
        };
        out.push((t, amp));
    }
    Ok(out)
}