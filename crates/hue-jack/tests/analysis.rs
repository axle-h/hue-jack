//! M3 acceptance: onsets, tempo, bands and silence on synthetic audio with ground truth.

use hue_jack::analysis::{FPS, FeatureFrame, analyze_mono};
use hue_jack::audio::testgen;

/// Matches detections to truth within ±25 ms. Returns (recall, false-positive rate, timing errors in ms).
fn score(truth: &[f64], detected: &[f64]) -> (f64, f64, Vec<f64>) {
    let mut used = vec![false; detected.len()];
    let mut errors = Vec::new();
    for &t in truth {
        let best = detected
            .iter()
            .enumerate()
            .filter(|(i, d)| !used[*i] && (*d - t).abs() <= 0.025)
            .min_by(|a, b| (a.1 - t).abs().total_cmp(&(b.1 - t).abs()));
        if let Some((i, d)) = best {
            used[i] = true;
            errors.push((d - t) * 1000.0);
        }
    }
    let recall = errors.len() as f64 / truth.len() as f64;
    let fp = used.iter().filter(|u| !**u).count() as f64 / detected.len().max(1) as f64;
    (recall, fp, errors)
}

fn steady_bpm(frames: &[FeatureFrame], after: f64) -> f32 {
    let mut bpms: Vec<f32> = frames
        .iter()
        .filter(|f| f.t > after)
        .filter_map(|f| f.bpm)
        .collect();
    assert!(!bpms.is_empty(), "no tempo estimate");
    bpms.sort_by(f32::total_cmp);
    let median = bpms[bpms.len() / 2];
    // Every estimate after warm-up stays close, not just the median.
    let off = bpms.iter().filter(|b| (**b - median).abs() > 1.0).count();
    assert!(
        off * 20 <= bpms.len(),
        "tempo unstable: {off}/{} estimates off the median {median}",
        bpms.len()
    );
    median
}

#[test]
fn clicks_120() {
    let (audio, truth) = testgen::clicks(30.0, 120.0);
    let frames = analyze_mono(&audio);
    let detected: Vec<f64> = frames.iter().filter_map(|f| f.onset_t).collect();
    let (recall, fp, errors) = score(&truth, &detected);
    let worst = errors.iter().fold(0.0f64, |m, e| m.max(e.abs()));
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    println!(
        "clicks: recall {recall:.3}, fp {fp:.3}, mean error {mean:+.2} ms, worst {worst:.2} ms"
    );
    assert!(recall >= 0.95, "recall {recall}");
    assert!(worst <= 10.0, "worst timing error {worst} ms");
    let bpm = steady_bpm(&frames, 8.0);
    assert!((bpm - 120.0).abs() <= 1.0, "bpm {bpm}");
}

#[test]
fn drums_128_kicks() {
    let (audio, truth) = testgen::drums(60.0, 128.0);
    let frames = analyze_mono(&audio);
    let detected: Vec<f64> = frames.iter().filter_map(|f| f.bass_onset_t).collect();
    let (recall, fp, errors) = score(&truth.kicks, &detected);
    let worst = errors.iter().fold(0.0f64, |m, e| m.max(e.abs()));
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    println!(
        "kicks: recall {recall:.3}, fp {fp:.3}, mean error {mean:+.2} ms, worst {worst:.2} ms ({} detected)",
        detected.len()
    );
    assert!(recall >= 0.90, "kick recall {recall}");
    assert!(fp <= 0.10, "false positives {fp}");
    let bpm = steady_bpm(&frames, 8.0);
    assert!((bpm - 128.0).abs() <= 1.0, "bpm {bpm}");
}

#[test]
fn sweep_moves_up_the_bands() {
    let frames = analyze_mono(&testgen::sweep(20.0));
    let dominant: Vec<usize> = frames
        .iter()
        .filter(|f| !f.silent && f.t > 0.1 && f.t < 19.9)
        .map(|f| {
            (0..5)
                .max_by(|&a, &b| f.band_db[a].total_cmp(&f.band_db[b]))
                .unwrap()
        })
        .collect();
    assert!(
        dominant.windows(2).all(|w| w[1] >= w[0]),
        "dominant band not monotonic: {dominant:?}"
    );
    assert_eq!(dominant.first(), Some(&0));
    assert_eq!(dominant.last(), Some(&4));
    // The AGC'd level follows too: the loudest normalised band is the dominant one most of the time.
    let agree = frames
        .iter()
        .filter(|f| !f.silent && f.t > 0.1 && f.t < 19.9)
        .filter(|f| {
            let raw = (0..5)
                .max_by(|&a, &b| f.band_db[a].total_cmp(&f.band_db[b]))
                .unwrap();
            f.bands[raw] >= 0.99 * f.bands.iter().cloned().fold(0.0, f32::max)
        })
        .count();
    assert!(
        agree as f64 >= 0.95 * dominant.len() as f64,
        "{agree}/{}",
        dominant.len()
    );
}

#[test]
fn silence_gap() {
    let (audio, (start, end)) = testgen::silence_gap(10.0, 30.0, 10.0);
    let frames = analyze_mono(&audio);
    let flagged = frames
        .iter()
        .find(|f| f.t > start && f.silent)
        .map(|f| f.t)
        .expect("silence flagged");
    let cleared = frames
        .iter()
        .find(|f| f.t > flagged && !f.silent)
        .map(|f| f.t)
        .expect("silence cleared");
    println!(
        "silence flagged {:.3} s after the gap starts, cleared {:.3} s after it ends",
        flagged - start,
        cleared - end
    );
    assert!(
        flagged - start <= 1.2,
        "flagged after {} s",
        flagged - start
    );
    assert!(
        (cleared - end).abs() <= 0.2,
        "cleared {} s from the end",
        cleared - end
    );
    assert!(
        frames
            .iter()
            .filter(|f| f.t > start + 1.2 && f.t < end - 0.05)
            .all(|f| f.silent)
    );
    assert!(
        frames
            .iter()
            .filter(|f| f.t < start - 0.1)
            .all(|f| !f.silent)
    );
    // No onsets reported during silence.
    assert!(
        frames
            .iter()
            .filter(|f| f.silent)
            .all(|f| f.onset == 0.0 && f.bass_onset == 0.0)
    );
}

#[test]
fn frame_rate() {
    let frames = analyze_mono(&vec![0.0; 48_000 * 5]);
    assert_eq!(FPS, 100);
    assert!((frames.len() as i64 - 500).abs() <= 6, "{}", frames.len());
    assert!(
        frames
            .windows(2)
            .all(|w| ((w[1].t - w[0].t) - 0.01).abs() < 1e-9)
    );
}

/// Run with `cargo test --release --test analysis -- --ignored perf`.
#[test]
#[ignore]
fn perf_60s_under_1s() {
    let (audio, _) = testgen::drums(60.0, 128.0);
    let stereo: Vec<f32> = audio.iter().flat_map(|&s| [s, s]).collect();
    let started = std::time::Instant::now();
    let frames = hue_jack::analysis::analyze_stereo(&stereo);
    let elapsed = started.elapsed();
    println!(
        "analysed 60 s ({} frames) in {:.0} ms",
        frames.len(),
        elapsed.as_secs_f64() * 1000.0
    );
    assert!(elapsed.as_secs_f64() < 1.0, "{elapsed:?}");
}
