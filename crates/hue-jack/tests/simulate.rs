//! M4 acceptance: `simulate` writes a valid preview for drums_128 with every effect.

use std::process::Command;

use hue_jack::effects::EFFECTS;

#[test]
fn simulate_drums_with_every_effect() {
    let dir = std::env::temp_dir().join(format!("hue-jack-simulate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_hue-jack");
    let status = Command::new(bin)
        .arg("gen-test-audio")
        .arg(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    for effect in EFFECTS {
        let out = dir.join(format!("preview-{effect}.html"));
        let status = Command::new(bin)
            .args([
                "simulate",
                dir.join("drums_128.wav").to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--effect",
                effect,
            ])
            .status()
            .unwrap();
        assert!(status.success(), "{effect}");
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("data:audio/wav;base64,UklGR"), "embedded WAV");
        let json: serde_json::Value =
            serde_json::from_str(hue_jack::simulate::extract_json(&html).unwrap()).unwrap();
        let duration = json["duration"].as_f64().unwrap();
        assert!((duration - 30.0).abs() < 1e-6, "clip is capped at 30 s");
        let frames = json["frames"].as_array().unwrap();
        assert!(
            (frames.len() as f64 - duration * 50.0).abs() <= 1.0,
            "{} frames",
            frames.len()
        );
        assert_eq!(frames[0].as_array().unwrap().len(), 6);
        assert!(
            frames.iter().any(|f| f
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c[0].as_u64().unwrap() > 128)),
            "{effect}: lights light up"
        );
        assert_eq!(json["effect"], *effect);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
