//! Named colour palettes. Effects index them with a fractional, wrapping position.

use serde::Serialize;

use super::Rgb;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Palette {
    pub name: &'static str,
    pub colors: &'static [[u8; 3]],
}

pub const PALETTES: &[Palette] = &[
    Palette {
        name: "sunset",
        colors: &[
            [255, 94, 58],
            [255, 149, 0],
            [255, 42, 104],
            [168, 50, 168],
            [255, 196, 70],
        ],
    },
    Palette {
        name: "ocean",
        colors: &[
            [0, 119, 255],
            [0, 210, 200],
            [40, 60, 255],
            [0, 255, 170],
            [90, 160, 255],
        ],
    },
    Palette {
        name: "neon",
        colors: &[
            [255, 0, 200],
            [0, 255, 255],
            [180, 0, 255],
            [0, 255, 80],
            [255, 240, 0],
        ],
    },
    Palette {
        name: "fire",
        colors: &[
            [255, 40, 0],
            [255, 120, 0],
            [255, 200, 30],
            [200, 20, 0],
            [255, 80, 10],
        ],
    },
    Palette {
        name: "forest",
        colors: &[
            [30, 200, 60],
            [140, 255, 40],
            [0, 140, 90],
            [200, 230, 60],
            [60, 255, 140],
        ],
    },
    Palette {
        name: "rainbow",
        colors: &[
            [255, 0, 0],
            [255, 160, 0],
            [255, 255, 0],
            [0, 255, 0],
            [0, 120, 255],
            [160, 0, 255],
        ],
    },
];

pub fn palette(name: &str) -> Option<&'static Palette> {
    PALETTES.iter().find(|p| p.name == name)
}

pub fn names() -> Vec<&'static str> {
    PALETTES.iter().map(|p| p.name).collect()
}

impl Palette {
    /// Colour at a fractional position (wraps; blends neighbouring entries).
    pub fn at(&self, pos: f32) -> Rgb {
        let n = self.colors.len() as f32;
        let p = pos.rem_euclid(n);
        let i = p.floor() as usize % self.colors.len();
        let j = (i + 1) % self.colors.len();
        let f = p - p.floor();
        let c = |k: usize, ch: usize| self.colors[k][ch] as f32 / 255.0;
        std::array::from_fn(|ch| c(i, ch) * (1.0 - f) + c(j, ch) * f)
    }

    /// `#rrggbb` strings for the API.
    pub fn hex(&self) -> Vec<String> {
        self.colors
            .iter()
            .map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_blends() {
        let p = palette("rainbow").unwrap();
        assert_eq!(p.at(0.0), [1.0, 0.0, 0.0]);
        assert_eq!(p.at(6.0), [1.0, 0.0, 0.0]);
        assert_eq!(p.at(-6.0), [1.0, 0.0, 0.0]);
        let mid = p.at(0.5);
        assert!((mid[1] - 80.0 / 255.0).abs() < 1e-6);
        assert_eq!(p.hex()[0], "#ff0000");
    }
}
