//! Portable transcendental functions (D17).
//!
//! `f32::sin`, `powf` and friends call the platform's C library (Apple libm,
//! the Windows UCRT, glibc), whose results differ in the last bit. Generated
//! geometry, colours and animation clips are hashed and committed, and saved
//! worlds regenerate them on load, so they must come out bit-identical on
//! every platform. These functions use the pure-Rust `libm` crate, which
//! gives the same result everywhere. glam uses the same crate (its `libm`
//! feature), so its rotation constructors and `slerp` are portable too.
//!
//! `sqrt`, `abs`, `floor`, `mul_add` and the arithmetic operators are exact
//! or correctly rounded by IEEE 754 and need no wrapper. `box_core`,
//! `box_proc` and `box_arch` forbid the platform versions through
//! `disallowed-methods` in their `clippy.toml`.

/// Sine (radians).
#[inline]
pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

/// Cosine (radians).
#[inline]
pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

/// Sine and cosine (radians).
#[inline]
pub fn sin_cos(x: f32) -> (f32, f32) {
    libm::sincosf(x)
}

/// Tangent (radians).
#[inline]
pub fn tan(x: f32) -> f32 {
    libm::tanf(x)
}

/// Arcsine, in radians.
#[inline]
pub fn asin(x: f32) -> f32 {
    libm::asinf(x)
}

/// Arccosine, in radians.
#[inline]
pub fn acos(x: f32) -> f32 {
    libm::acosf(x)
}

/// Arctangent, in radians.
#[inline]
pub fn atan(x: f32) -> f32 {
    libm::atanf(x)
}

/// Four-quadrant arctangent of `y / x`, in radians.
#[inline]
pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

/// `x` raised to the power `y`.
#[inline]
pub fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}

/// `e^x`.
#[inline]
pub fn exp(x: f32) -> f32 {
    libm::expf(x)
}

/// Natural logarithm.
#[inline]
pub fn ln(x: f32) -> f32 {
    libm::logf(x)
}

/// Portable `f64` versions.
pub mod f64 {
    /// Sine (radians).
    #[inline]
    pub fn sin(x: f64) -> f64 {
        libm::sin(x)
    }

    /// Cosine (radians).
    #[inline]
    pub fn cos(x: f64) -> f64 {
        libm::cos(x)
    }

    /// Sine and cosine (radians).
    #[inline]
    pub fn sin_cos(x: f64) -> (f64, f64) {
        libm::sincos(x)
    }

    /// Tangent (radians).
    #[inline]
    pub fn tan(x: f64) -> f64 {
        libm::tan(x)
    }

    /// Arccosine, in radians.
    #[inline]
    pub fn acos(x: f64) -> f64 {
        libm::acos(x)
    }

    /// Four-quadrant arctangent of `y / x`, in radians.
    #[inline]
    pub fn atan2(y: f64, x: f64) -> f64 {
        libm::atan2(y, x)
    }

    /// `x` raised to the power `y`.
    #[inline]
    pub fn powf(x: f64, y: f64) -> f64 {
        libm::pow(x, y)
    }

    /// `e^x`.
    #[inline]
    pub fn exp(x: f64) -> f64 {
        libm::exp(x)
    }

    /// Natural logarithm.
    #[inline]
    pub fn ln(x: f64) -> f64 {
        libm::log(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values whose platform results differ (the sRGB curve at 0.95 lies
    /// 3e-11 from an f32 rounding midpoint: Apple's powf rounds it down,
    /// the Windows UCRT up). Pinned bits, the same on every platform; libm
    /// rounds this one down.
    #[test]
    fn results_are_pinned() {
        let t = (0.95_f32 + 0.055) / 1.055;
        assert_eq!(powf(t, 2.4).to_bits(), 0x3f63_d766);
        assert_eq!(sin(1.0).to_bits(), 0x3f57_6aa4);
        assert_eq!(cos(1.0).to_bits(), 0x3f0a_5140);
        assert_eq!(atan2(1.0, 2.0).to_bits(), 0x3eed_6338);
    }
}
