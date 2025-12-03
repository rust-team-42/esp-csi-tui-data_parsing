use core::f32;
use std::{error::Error, fs};
use color_eyre::Result;
use csv;
//use rerun::external::arrow::csv;
use std::fs::File;
use std::io::BufReader;

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

pub fn load_csv_heatmap(path: &str) -> Result<Vec<Vec<u8>>> {
    let file = File::open(path)?;
    let mut rdr = csv::Reader::from_reader(BufReader::new(file));

    let headers = rdr.headers()?.clone();
    let total_cols = headers.len();

    // We expect at least: timestamp, rssi, i0, q0
    if total_cols < 4 {
        return Ok(Vec::new());
    }

    // After the first two columns (timestamp, rssi), all remaining columns are interleaved I/Q:
    // i0,q0,i1,q1,..., so there should be an even number of them.
    let num_iq_cols = total_cols - 2;
    let mut num_subcarriers = num_iq_cols / 2;

    // If odd (shouldn't happen), drop the last stray column.
    if num_iq_cols % 2 != 0 {
        num_subcarriers -= 1;
    }

    if num_subcarriers == 0 {
        return Ok(Vec::new());
    }

    // First pass: compute raw amplitudes and track global min/max.
    let mut raw_amp_rows: Vec<Vec<f32>> = Vec::new();
    let mut global_min = f32::INFINITY;
    let mut global_max = f32::NEG_INFINITY;

    for result in rdr.records() {
        let record = result?;

        let mut amps_for_row = Vec::with_capacity(num_subcarriers);
        for sc in 0..num_subcarriers {
            // Column layout: 0: ts, 1: rssi, 2: i0, 3: q0, 4: i1, 5: q1, ...
            let i_idx = 2 + 2 * sc;
            let q_idx = 2 + 2 * sc + 1;

            let i_val: f32 = record
                .get(i_idx)
                .unwrap_or("0")
                .trim()
                .parse()
                .unwrap_or(0.0);
            let q_val: f32 = record
                .get(q_idx)
                .unwrap_or("0")
                .trim()
                .parse()
                .unwrap_or(0.0);

            // Your equation (no sqrt): A_k(t_i) = I_k^2 + Q_k^2
            let a_sq = i_val * i_val + q_val * q_val;

            global_min = global_min.min(a_sq);
            global_max = global_max.max(a_sq);
            amps_for_row.push(a_sq);
        }

        raw_amp_rows.push(amps_for_row);
    }

    if raw_amp_rows.is_empty() {
        return Ok(Vec::new());
    }

    // Guard against degenerate case (all amplitudes identical, NaN, etc.)
    if !global_min.is_finite() || !global_max.is_finite() || global_max <= global_min {
        let rows = raw_amp_rows.len();
        let cols = num_subcarriers;
        return Ok(vec![vec![0u8; cols]; rows]);
    }

    // Second pass: normalize to 0–100.
    let range = global_max - global_min;
    let mut heatmap: Vec<Vec<u8>> = Vec::with_capacity(raw_amp_rows.len());

    for row in raw_amp_rows.into_iter() {
        let mut out_row = Vec::with_capacity(row.len());
        for a_sq in row.into_iter() {
            let norm = (a_sq - global_min) / range; // 0.0 .. 1.0
            let clamped = norm.clamp(0.0, 1.0);
            let scaled = (clamped * 100.0).round() as u8; // 0 .. 100
            out_row.push(scaled);
        }
        heatmap.push(out_row);
    }

    Ok(heatmap)
}