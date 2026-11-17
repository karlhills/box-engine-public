//! Custom shader materials: [`ShaderMaterial`] (saved data), [`ShaderSource`]
//! (the loaded `.wgsl` text, an asset) and the parameter header parser
//! [`parse_material_header`].
//!
//! A material shader is a WGSL file that defines `fn surface` (and optionally
//! `fn vertex`); the renderer wraps it with the engine's shader library and
//! lights the returned `Surface` (see `box_render`'s documentation). Its
//! editable parameters are declared once, in a header:
//!
//! ```wgsl
//! @material struct Params {
//!     /// Stripe colour (linear).
//!     @color tint: vec3f = vec3f(0.9, 0.3, 0.2),
//!     /// Stripes per metre.
//!     @range(1.0, 40.0) frequency: f32 = 10.0,
//!     offset: vec2f,
//! }
//! ```
//!
//! Header syntax: `@material struct <Name> { <field>, ... }`, at most one per
//! file. Each field is `[/// doc]* [@range(min, max)] [@color] name: type
//! [= default]`. Types: `f32`, `i32`, `u32`, `vec2f`, `vec3f`, `vec4f` (or
//! `vecN<f32>`). `@color` marks a `vec3f`/`vec4f` as a linear colour. A
//! default is a number or `vecNf(a, b, ...)` / `(a, b, ...)` with one value
//! per component, or one value for all; without one the default is zero.
//! Doc comments become tooltips. The renderer replaces the header with a
//! plain WGSL struct of the same name bound as `material` (group 2).

use std::fmt;
use std::ops::RangeInclusive;

use glam::{Vec2, Vec3, Vec4};

use crate::asset_ref::AssetRef;
use crate::color::Color;
use crate::material::{AlphaMode, DepthWrite, yes};
use crate::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};

/// A material drawn by a custom WGSL shader with editable parameters.
///
/// A scene component (on an entity with a `Primitive` it is that entity's
/// material and wins over a `StandardMaterial`) and the content of material
/// assets. Parameters are matched to the shader header by name, so values
/// survive reordering the header; values for parameters the header does not
/// declare are kept but unused.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ShaderMaterial {
    /// The material's `.wgsl` file. While it is not loaded the surface draws
    /// with the default material; a shader that fails to compile draws with
    /// the magenta error material.
    pub shader: AssetRef,
    /// Parameter values by name. A parameter missing here, or holding a
    /// value of the wrong type, uses the default from the shader header.
    #[reflect(default)]
    pub params: Vec<ShaderParam>,
    /// How the alpha returned by the shader is used.
    #[reflect(default)]
    pub alpha_mode: AlphaMode,
    /// Draw both faces; back faces get flipped normals.
    #[reflect(default)]
    pub double_sided: bool,
    /// Transparent modes: fade out over this distance in front of the opaque
    /// surface behind (soft particles). 0 is off. See
    /// `StandardMaterial::depth_fade`.
    #[reflect(default, @Range { min: 0.0, max: 50.0 }, @Unit("m"), @Step(0.05))]
    pub depth_fade: f32,
    /// Transparent modes: draw order. See `StandardMaterial::sort_bias`.
    #[reflect(default)]
    pub sort_bias: i32,
    /// Whether the material writes depth.
    #[reflect(default)]
    pub depth_write: DepthWrite,
    /// Test against depth. Off draws over everything drawn before it.
    #[reflect(default = "yes")]
    pub depth_test: bool,
    /// Transparent modes: write depth first, then draw colour only on the
    /// nearest surface (a ghost look). See `StandardMaterial::prewrite_depth`.
    #[reflect(default)]
    pub prewrite_depth: bool,
    /// The shader reads the scene depth (`scene_depth` in material_api):
    /// run the depth prepass while the material is visible. A shader that
    /// calls `scene_depth` requests it without this.
    #[reflect(default)]
    pub scene_depth: bool,
    /// Margin added on every side of the culling bounds of meshes drawn
    /// with this material, for a `vertex` hook that moves vertices outside
    /// the mesh (waves, sway): without it they can disappear at the edges of
    /// the view or from shadows while still on screen.
    #[reflect(default, @Range { min: 0.0, max: 100.0 }, @Unit("m"), @Step(0.05))]
    pub bounds_padding: f32,
}

impl Default for ShaderMaterial {
    fn default() -> Self {
        Self {
            shader: AssetRef::NONE,
            params: Vec::new(),
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
            depth_fade: 0.0,
            sort_bias: 0,
            depth_write: DepthWrite::Auto,
            depth_test: true,
            prewrite_depth: false,
            scene_depth: false,
            bounds_padding: 0.0,
        }
    }
}

/// One named parameter value of a [`ShaderMaterial`].
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ShaderParam {
    /// Field name in the shader's `@material` struct.
    pub name: String,
    pub value: ShaderParamValue,
}

/// A parameter value. Colours are linear.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum ShaderParamValue {
    Float(f32),
    Int(i32),
    Uint(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    Color(Color),
}

impl Default for ShaderParamValue {
    fn default() -> Self {
        Self::Float(0.0)
    }
}

impl ShaderParamValue {
    /// The value as four 32-bit words in WGSL order (unused words zero):
    /// float bits for floats and colours (rgba), two's complement for `i32`.
    /// This is the uniform-buffer representation, and also a NaN-safe
    /// identity for change detection.
    pub fn words(&self) -> [u32; 4] {
        let f = |v: &[f32]| {
            let mut w = [0u32; 4];
            for (o, x) in w.iter_mut().zip(v) {
                *o = x.to_bits();
            }
            w
        };
        match *self {
            Self::Float(v) => f(&[v]),
            Self::Int(v) => [v as u32, 0, 0, 0],
            Self::Uint(v) => [v, 0, 0, 0],
            Self::Vec2(v) => f(&v.to_array()),
            Self::Vec3(v) => f(&v.to_array()),
            Self::Vec4(v) => f(&v.to_array()),
            Self::Color(c) => f(&c.to_array()),
        }
    }
}

impl ShaderMaterial {
    /// A material using the shader at `shader`, with every parameter at its
    /// header default.
    pub fn new(shader: AssetRef) -> Self {
        Self {
            shader,
            ..Self::default()
        }
    }

    /// Sets parameter `name`, replacing an earlier value.
    pub fn with_param(mut self, name: &str, value: ShaderParamValue) -> Self {
        match self.params.iter_mut().find(|p| p.name == name) {
            Some(p) => p.value = value,
            None => self.params.push(ShaderParam {
                name: name.to_string(),
                value,
            }),
        }
        self
    }

    /// The value the shader receives for `decl`: this material's value when
    /// it has one the declaration accepts, otherwise the header default.
    pub fn value_for(&self, decl: &ShaderParamDecl) -> ShaderParamValue {
        self.params
            .iter()
            .find(|p| p.name == decl.name)
            .map(|p| p.value)
            .filter(|v| decl.ty.accepts(v))
            .unwrap_or(decl.default)
    }
}

/// The text of a material shader: what a `.wgsl` asset loads into. The asset
/// server keeps these in `Assets<ShaderSource>` and maps their [`AssetRef`]s
/// in `AssetLookup`; editing the file replaces the value (bumping the asset
/// version), which the renderer recompiles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShaderSource {
    /// The WGSL source.
    pub text: String,
    /// Name used in error messages (the asset path).
    pub name: String,
}

impl ShaderSource {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            name: name.into(),
        }
    }
}

/// The type of a declared parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderParamType {
    Float,
    Int,
    Uint,
    Vec2,
    Vec3,
    Vec4,
    /// `@color vec3f`: linear RGB.
    Rgb,
    /// `@color vec4f`: linear RGBA.
    Rgba,
}

impl ShaderParamType {
    /// The WGSL type.
    pub fn wgsl(self) -> &'static str {
        match self {
            Self::Float => "f32",
            Self::Int => "i32",
            Self::Uint => "u32",
            Self::Vec2 => "vec2<f32>",
            Self::Vec3 | Self::Rgb => "vec3<f32>",
            Self::Vec4 | Self::Rgba => "vec4<f32>",
        }
    }

    /// Number of scalar components.
    pub fn components(self) -> usize {
        match self {
            Self::Float | Self::Int | Self::Uint => 1,
            Self::Vec2 => 2,
            Self::Vec3 | Self::Rgb => 3,
            Self::Vec4 | Self::Rgba => 4,
        }
    }

    /// Whether `value` can be bound to a parameter of this type. Colours
    /// also accept a `Vec3`/`Vec4` of the same size.
    pub fn accepts(self, value: &ShaderParamValue) -> bool {
        use ShaderParamValue as V;
        matches!(
            (self, value),
            (Self::Float, V::Float(_))
                | (Self::Int, V::Int(_))
                | (Self::Uint, V::Uint(_))
                | (Self::Vec2, V::Vec2(_))
                | (Self::Vec3, V::Vec3(_))
                | (Self::Vec4, V::Vec4(_))
                | (Self::Rgb, V::Color(_) | V::Vec3(_))
                | (Self::Rgba, V::Color(_) | V::Vec4(_))
        )
    }

    fn value(self, c: [f64; 4]) -> ShaderParamValue {
        use ShaderParamValue as V;
        let f = c.map(|x| x as f32);
        match self {
            Self::Float => V::Float(f[0]),
            Self::Int => V::Int(c[0] as i32),
            Self::Uint => V::Uint(c[0] as u32),
            Self::Vec2 => V::Vec2(Vec2::new(f[0], f[1])),
            Self::Vec3 => V::Vec3(Vec3::new(f[0], f[1], f[2])),
            Self::Vec4 => V::Vec4(Vec4::from_array(f)),
            Self::Rgb => V::Color(Color::linear(f[0], f[1], f[2])),
            Self::Rgba => V::Color(Color::linear_rgba(f[0], f[1], f[2], f[3])),
        }
    }
}

/// One parameter declared in a shader header: what an inspector shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderParamDecl {
    pub name: String,
    pub ty: ShaderParamType,
    pub default: ShaderParamValue,
    /// `@range(min, max)`, applied per component.
    pub range: Option<(f32, f32)>,
    /// The `///` doc comment (tooltip), lines joined with spaces.
    pub doc: Option<String>,
}

/// A parsed `@material struct` header.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialHeader {
    /// The struct's name; user hooks take a parameter of this type.
    pub struct_name: String,
    pub params: Vec<ShaderParamDecl>,
    /// Source lines (1-based) the header occupies, from `@material` to the
    /// closing brace (or semicolon).
    pub lines: RangeInclusive<usize>,
}

impl MaterialHeader {
    /// The header as plain WGSL: a struct with the same name and fields.
    /// WGSL has no empty structs, so a header without fields gets one
    /// unused `vec4<f32>` padding member.
    pub fn wgsl_struct(&self) -> String {
        let mut s = format!("struct {} {{\n", self.struct_name);
        if self.params.is_empty() {
            s.push_str("    _unused: vec4<f32>,\n");
        }
        for p in &self.params {
            s.push_str(&format!("    {}: {},\n", p.name, p.ty.wgsl()));
        }
        s.push_str("};\n");
        s
    }

    /// A parameter by name.
    pub fn param(&self, name: &str) -> Option<&ShaderParamDecl> {
        self.params.iter().find(|p| p.name == name)
    }
}

/// A malformed header, with the 1-based source line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for HeaderError {}

/// Finds and parses the `@material struct` header of a material shader.
/// `Ok(None)` when the file has none (the shader has no parameters).
pub fn parse_material_header(source: &str) -> Result<Option<MaterialHeader>, HeaderError> {
    let tokens = tokenize(source)?;
    let mut found = None;
    for (i, w) in tokens.windows(2).enumerate() {
        if w[0].kind == Tok::Punct('@') && w[1].kind == Tok::Ident("material".into()) {
            if found.is_some() {
                return Err(HeaderError {
                    line: w[0].line,
                    message: "more than one @material header".into(),
                });
            }
            found = Some(i);
        }
    }
    let Some(start) = found else {
        return Ok(None);
    };
    let mut p = Parser {
        tokens: &tokens,
        pos: start + 2,
        line: tokens[start].line,
    };
    p.expect_ident("struct")?;
    let struct_name = p.ident("the struct name")?;
    p.expect('{')?;
    let mut params: Vec<ShaderParamDecl> = Vec::new();
    loop {
        let mut doc: Vec<String> = Vec::new();
        while let Some(Tok::Doc(d)) = p.peek() {
            doc.push(d.clone());
            p.pos += 1;
        }
        if p.eat('}') {
            break;
        }
        let decl = p.field(doc)?;
        if params.iter().any(|q| q.name == decl.name) {
            return p.fail(format!("duplicate parameter `{}`", decl.name));
        }
        params.push(decl);
        if !p.eat(',') {
            p.expect('}')?;
            break;
        }
    }
    let mut end = p.line;
    if p.peek() == Some(&Tok::Punct(';')) {
        end = p.tokens[p.pos].line;
    }
    Ok(Some(MaterialHeader {
        struct_name,
        params,
        lines: tokens[start].line..=end,
    }))
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Number(String),
    Punct(char),
    /// A `///` comment's text.
    Doc(String),
}

struct Token {
    kind: Tok,
    line: usize,
}

fn tokenize(src: &str) -> Result<Vec<Token>, HeaderError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            if let Some(doc) = text.strip_prefix("///")
                && !doc.starts_with('/')
            {
                out.push(Token {
                    kind: Tok::Doc(doc.trim().to_string()),
                    line,
                });
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            // WGSL block comments nest.
            let (open_line, mut depth) = (line, 0);
            loop {
                if i >= chars.len() {
                    return Err(HeaderError {
                        line: open_line,
                        message: "unterminated block comment".into(),
                    });
                }
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Token {
                kind: Tok::Ident(chars[start..i].iter().collect()),
                line,
            });
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = i;
            while i < chars.len() {
                let d = chars[i];
                let exponent_sign = (d == '-' || d == '+')
                    && matches!(chars[i - 1], 'e' | 'E')
                    && !chars[start..i].iter().any(|&x| x == 'x' || x == 'X');
                if d.is_ascii_alphanumeric() || d == '.' || exponent_sign {
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Token {
                kind: Tok::Number(chars[start..i].iter().collect()),
                line,
            });
        } else {
            out.push(Token {
                kind: Tok::Punct(c),
                line,
            });
            i += 1;
        }
    }
    Ok(out)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Line of the last token consumed (for errors at the end of input).
    line: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.kind)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos)?;
        self.pos += 1;
        self.line = t.line;
        Some(t.kind.clone())
    }

    fn fail<T>(&self, message: String) -> Result<T, HeaderError> {
        let line = self.tokens.get(self.pos).map_or(self.line, |t| t.line);
        Err(HeaderError { line, message })
    }

    fn describe(&self) -> String {
        match self.peek() {
            None => "the end of the file".into(),
            Some(Tok::Ident(s)) => format!("`{s}`"),
            Some(Tok::Number(s)) => format!("`{s}`"),
            Some(Tok::Punct(c)) => format!("`{c}`"),
            Some(Tok::Doc(_)) => "a doc comment".into(),
        }
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), HeaderError> {
        if self.eat(c) {
            Ok(())
        } else {
            self.fail(format!("expected `{c}`, found {}", self.describe()))
        }
    }

    fn ident(&mut self, what: &str) -> Result<String, HeaderError> {
        match self.peek() {
            Some(Tok::Ident(s)) => {
                let s = s.clone();
                self.next();
                Ok(s)
            }
            _ => self.fail(format!("expected {what}, found {}", self.describe())),
        }
    }

    fn expect_ident(&mut self, word: &str) -> Result<(), HeaderError> {
        match self.peek() {
            Some(Tok::Ident(s)) if s == word => {
                self.next();
                Ok(())
            }
            _ => self.fail(format!(
                "expected `{word}` after @material, found {}",
                self.describe()
            )),
        }
    }

    /// A possibly negative number literal (WGSL suffixes allowed).
    fn number(&mut self) -> Result<f64, HeaderError> {
        let negative = self.eat('-');
        let Some(Tok::Number(text)) = self.peek().cloned() else {
            return self.fail(format!("expected a number, found {}", self.describe()));
        };
        let trimmed = text.trim_end_matches(['f', 'h', 'i', 'u']);
        let value = if let Some(hex) = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
        {
            i64::from_str_radix(hex, 16).ok().map(|v| v as f64)
        } else {
            trimmed.parse::<f64>().ok()
        };
        let Some(v) = value.filter(|v| v.is_finite()) else {
            return self.fail(format!("`{text}` is not a number"));
        };
        self.next();
        Ok(if negative { -v } else { v })
    }

    fn field(&mut self, doc: Vec<String>) -> Result<ShaderParamDecl, HeaderError> {
        let (mut range, mut color) = (None, false);
        while self.eat('@') {
            let attr = self.ident("an attribute name")?;
            match attr.as_str() {
                "range" => {
                    self.expect('(')?;
                    let min = self.number()? as f32;
                    self.expect(',')?;
                    let max = self.number()? as f32;
                    self.eat(',');
                    self.expect(')')?;
                    if min > max {
                        return self.fail(format!("@range({min}, {max}): min is above max"));
                    }
                    range = Some((min, max));
                }
                "color" => color = true,
                other => {
                    return self.fail(format!(
                        "unknown attribute `@{other}` (expected @range or @color)"
                    ));
                }
            }
        }
        let name = self.ident("a parameter name")?;
        self.expect(':')?;
        let ty_name = self.ident("a type")?;
        let mut ty = match ty_name.as_str() {
            "f32" => ShaderParamType::Float,
            "i32" => ShaderParamType::Int,
            "u32" => ShaderParamType::Uint,
            "vec2f" => ShaderParamType::Vec2,
            "vec3f" => ShaderParamType::Vec3,
            "vec4f" => ShaderParamType::Vec4,
            "vec2" | "vec3" | "vec4" => {
                self.expect('<')?;
                let inner = self.ident("`f32`")?;
                if inner != "f32" {
                    return self.fail(format!(
                        "`{ty_name}<{inner}>`: only f32 vectors are supported"
                    ));
                }
                self.expect('>')?;
                match ty_name.as_str() {
                    "vec2" => ShaderParamType::Vec2,
                    "vec3" => ShaderParamType::Vec3,
                    _ => ShaderParamType::Vec4,
                }
            }
            other => {
                return self.fail(format!(
                    "parameter `{name}` has unsupported type `{other}` \
                     (use f32, i32, u32, vec2f, vec3f or vec4f)"
                ));
            }
        };
        if color {
            ty = match ty {
                ShaderParamType::Vec3 => ShaderParamType::Rgb,
                ShaderParamType::Vec4 => ShaderParamType::Rgba,
                _ => {
                    return self.fail(format!(
                        "@color needs a vec3f or vec4f, but `{name}` is {ty_name}"
                    ));
                }
            };
        }
        let default = if self.eat('=') {
            self.default_value(&name, ty)?
        } else {
            ty.value([0.0; 4])
        };
        Ok(ShaderParamDecl {
            name,
            ty,
            default,
            range,
            doc: (!doc.is_empty()).then(|| doc.join(" ")),
        })
    }

    fn default_value(
        &mut self,
        name: &str,
        ty: ShaderParamType,
    ) -> Result<ShaderParamValue, HeaderError> {
        let mut values = Vec::new();
        let constructor = matches!(self.peek(), Some(Tok::Ident(_)));
        if constructor {
            let ctor = self.ident("a constructor")?;
            if !matches!(
                ctor.as_str(),
                "vec2f" | "vec3f" | "vec4f" | "vec2" | "vec3" | "vec4" | "f32" | "i32" | "u32"
            ) {
                return self.fail(format!("unsupported default `{ctor}(...)`"));
            }
            if self.eat('<') {
                self.ident("a type")?;
                self.expect('>')?;
            }
        }
        if constructor || self.peek() == Some(&Tok::Punct('(')) {
            self.expect('(')?;
            loop {
                values.push(self.number()?);
                if !self.eat(',') || self.peek() == Some(&Tok::Punct(')')) {
                    break;
                }
            }
            self.expect(')')?;
        } else {
            values.push(self.number()?);
        }
        let n = ty.components();
        let mut c = [0.0f64; 4];
        match values.len() {
            1 => c[..n].fill(values[0]),
            len if len == n => c[..n].copy_from_slice(&values),
            len => {
                return self.fail(format!(
                    "default of `{name}` has {len} values, its type needs {n}"
                ));
            }
        }
        if matches!(ty, ShaderParamType::Int | ShaderParamType::Uint) && c[0].fract() != 0.0 {
            return self.fail(format!("default of `{name}` must be an integer"));
        }
        if ty == ShaderParamType::Uint && c[0] < 0.0 {
            return self.fail(format!("default of `{name}` must not be negative"));
        }
        Ok(ty.value(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::Registry;

    const STRIPES: &str = r#"// A striped material.
#import box::lighting

@material struct Params {
    /// Stripe colour
    /// (linear).
    @color tint: vec3f = vec3f(0.9, 0.3, 0.2),
    @range(1.0, 40.0) frequency: f32 = 10.0,
    offset: vec2<f32> = (0.5, -1.5e-1),
    count: u32 = 3u,
    glow: vec4f = 0.25, // splat
}

fn surface(in: SurfaceInput, p: Params) -> Surface {
    return default_surface(in); // @material in a comment is ignored
}
"#;

    #[test]
    fn parses_fields_defaults_ranges_and_docs() {
        let h = parse_material_header(STRIPES).unwrap().unwrap();
        assert_eq!(h.struct_name, "Params");
        assert_eq!(h.lines, 4..=12);
        let names: Vec<_> = h.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["tint", "frequency", "offset", "count", "glow"]);
        let tint = h.param("tint").unwrap();
        assert_eq!(tint.ty, ShaderParamType::Rgb);
        assert_eq!(
            tint.default,
            ShaderParamValue::Color(Color::linear(0.9, 0.3, 0.2))
        );
        assert_eq!(tint.doc.as_deref(), Some("Stripe colour (linear)."));
        let freq = h.param("frequency").unwrap();
        assert_eq!(freq.range, Some((1.0, 40.0)));
        assert_eq!(freq.default, ShaderParamValue::Float(10.0));
        assert_eq!(freq.doc, None);
        assert_eq!(
            h.param("offset").unwrap().default,
            ShaderParamValue::Vec2(Vec2::new(0.5, -0.15))
        );
        assert_eq!(h.param("count").unwrap().default, ShaderParamValue::Uint(3));
        assert_eq!(
            h.param("glow").unwrap().default,
            ShaderParamValue::Vec4(Vec4::splat(0.25))
        );
        assert_eq!(
            h.wgsl_struct(),
            "struct Params {\n    tint: vec3<f32>,\n    frequency: f32,\n    \
             offset: vec2<f32>,\n    count: u32,\n    glow: vec4<f32>,\n};\n"
        );
    }

    #[test]
    fn no_header_and_empty_header() {
        assert_eq!(parse_material_header("fn surface() {}"), Ok(None));
        let h = parse_material_header("@material struct P {}")
            .unwrap()
            .unwrap();
        assert!(h.params.is_empty());
        assert!(h.wgsl_struct().contains("_unused"));
        // `@material` inside a block comment is not a header.
        assert_eq!(
            parse_material_header("/* @material struct P { a: f32 } */"),
            Ok(None)
        );
    }

    #[test]
    fn malformed_headers_report_the_line() {
        let err = |src: &str| parse_material_header(src).unwrap_err();
        let e = err("\n@material Params { a: f32 }");
        assert_eq!(e.line, 2);
        assert!(e.message.contains("expected `struct`"), "{e}");
        assert!(
            err("@material struct P { a: mat4x4f }")
                .message
                .contains("unsupported type")
        );
        assert!(
            err("@material struct P { a: vec3<i32> }")
                .message
                .contains("only f32")
        );
        assert!(
            err("@material struct P { @color a: f32 }")
                .message
                .contains("@color")
        );
        assert!(
            err("@material struct P { @bogus a: f32 }")
                .message
                .contains("unknown attribute")
        );
        assert!(
            err("@material struct P { a: f32, a: f32 }")
                .message
                .contains("duplicate")
        );
        assert!(
            err("@material struct P { a: vec3f = (1, 2) }")
                .message
                .contains("2 values")
        );
        assert!(
            err("@material struct P { a: f32 = x }")
                .message
                .contains("unsupported default")
        );
        assert!(
            err("@material struct P { a: u32 = -1 }")
                .message
                .contains("negative")
        );
        assert!(
            err("@material struct P { a: i32 = 1.5 }")
                .message
                .contains("integer")
        );
        assert!(
            err("@material struct P { @range(2, 1) a: f32 }")
                .message
                .contains("above max")
        );
        assert!(
            err("@material struct P { a f32 }")
                .message
                .contains("expected `:`")
        );
        let e = err("@material struct P {\n a: f32\n b: f32 }");
        assert_eq!(e.line, 3);
        let e = err("@material struct P {\n a: f32,\n");
        assert!(e.message.contains("the end of the file"), "{e}");
        assert!(
            err("@material struct A {}\n@material struct B {}")
                .message
                .contains("more than one")
        );
        assert!(err("/* open").message.contains("unterminated"));
    }

    #[test]
    fn values_fall_back_to_defaults_by_name_and_type() {
        let h = parse_material_header(STRIPES).unwrap().unwrap();
        let m = ShaderMaterial::new(AssetRef::path("shaders/stripes.wgsl"))
            .with_param("frequency", ShaderParamValue::Float(4.0))
            .with_param("tint", ShaderParamValue::Float(1.0)) // wrong type
            .with_param("unknown", ShaderParamValue::Int(1))
            .with_param("frequency", ShaderParamValue::Float(5.0));
        assert_eq!(m.params.len(), 3);
        let freq = h.param("frequency").unwrap();
        assert_eq!(m.value_for(freq), ShaderParamValue::Float(5.0));
        let tint = h.param("tint").unwrap();
        assert_eq!(m.value_for(tint), tint.default);
        let offset = h.param("offset").unwrap();
        assert_eq!(m.value_for(offset), offset.default);
        assert_eq!(
            ShaderParamValue::Int(-1).words(),
            [u32::MAX, 0, 0, 0],
            "two's complement"
        );
    }

    #[test]
    fn shader_material_round_trips_through_scene_json() {
        let registry = Registry::with_core_types();
        let m = ShaderMaterial {
            shader: AssetRef::path("shaders/stripes.wgsl"),
            params: vec![
                ShaderParam {
                    name: "tint".into(),
                    value: ShaderParamValue::Color(Color::linear(0.1, 0.2, 0.3)),
                },
                ShaderParam {
                    name: "frequency".into(),
                    value: ShaderParamValue::Float(12.0),
                },
            ],
            alpha_mode: AlphaMode::Multiply2x,
            double_sided: true,
            depth_fade: 1.5,
            sort_bias: -2,
            depth_write: DepthWrite::On,
            depth_test: false,
            prewrite_depth: true,
            scene_depth: true,
            bounds_padding: 0.25,
        };
        let json = registry.value_to_json(&m).unwrap();
        assert_eq!(json["alpha_mode"], serde_json::json!("Multiply2x"));
        assert_eq!(json["depth_write"], serde_json::json!("On"));
        assert_eq!(
            json["shader"],
            serde_json::json!({"path": "shaders/stripes.wgsl"})
        );
        assert_eq!(
            json["params"][1]["value"],
            serde_json::json!({"Float": 12.0})
        );
        let ty = registry.component("ShaderMaterial").unwrap();
        let back = registry.value_from_json(ty.type_id(), &json).unwrap();
        assert_eq!(
            <ShaderMaterial as bevy_reflect::FromReflect>::from_reflect(back.as_ref()).unwrap(),
            m
        );
        // Only the shader is required.
        let minimal = serde_json::json!({"shader": "shaders/stripes.wgsl"});
        let v = registry.value_from_json(ty.type_id(), &minimal).unwrap();
        let m = <ShaderMaterial as bevy_reflect::FromReflect>::from_reflect(v.as_ref()).unwrap();
        assert!(m.params.is_empty() && m.alpha_mode == AlphaMode::Opaque);
        // Files from before the pipeline state fields load with the old
        // behaviour.
        assert_eq!(
            m,
            ShaderMaterial::new(AssetRef::path("shaders/stripes.wgsl"))
        );
        assert!(m.depth_test && !m.scene_depth && m.bounds_padding == 0.0);
    }
}
