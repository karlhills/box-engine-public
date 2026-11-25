# Materials

How surfaces look: the built-in `StandardMaterial`, material shaders (`ShaderMaterial`, your own WGSL), alpha modes, draw order and depth, and reading the scene depth from a shader.

A material is either a component next to a `Primitive` or a generator, or a material file (`assets/**/*.material.json`) that objects link to. Both hold the same fields; the inspector shows them, and a file is saved as you edit it.

## Standard materials

A `.material.json` without a `"shader"` key is a `StandardMaterial`: base colour, roughness, metallic, emissive, four optional textures (base colour, normal, occlusion/roughness/metallic, emissive), `uv_scale`, wind sway, and the fields below. Every field is optional; a missing one takes its default.

```json
{
  "base_color": [0.55, 0.85, 1.0, 0.35],
  "alpha_mode": "Blend",
  "double_sided": true,
  "prewrite_depth": true
}
```

## Alpha modes

| `alpha_mode` | What it does |
|---|---|
| `"Opaque"` (default) | Alpha is ignored. |
| `{ "Mask": { "cutoff": 0.5 } }` | Pixels with alpha below the cutoff are cut out, also from shadows. |
| `"Blend"` | See-through, blended by alpha, drawn back to front after opaque surfaces. |
| `"Additive"` | Adds light: glows, sparks, beams. |
| `"Multiply"` | Multiplies what is behind: tinted glass, dark stains. Unlit. |
| `"Multiply2x"` | Multiplies what is behind by twice its colour: mid grey (0.5) changes nothing, darker darkens, lighter lightens. Grime, dirt and wear decals that both darken and brighten. Unlit. |

The last four are the transparent modes. They do not cast shadows and, by default, do not write depth.

## Draw order and depth

These fields change how a material is drawn, not how it is lit. Their defaults are what you get without them.

| Field | Default | What it does |
|---|---|---|
| `sort_bias` | `0` | Transparent modes: a higher bias draws later, over transparent surfaces with a lower bias, whatever their distance. Equal biases sort back to front. Use it when two sheets fight (water over a puddle decal, a glow that must stay in front of smoke). |
| `depth_write` | `"Auto"` | `"Auto"`: opaque and masked materials write depth, transparent ones do not. `"On"`: always write, so a transparent surface hides transparent surfaces drawn after it and behind it. `"Off"`: never write (an opaque material then also stays out of ambient occlusion). |
| `depth_test` | `true` | `false` draws the surface over everything drawn before it: markers that show through walls. |
| `prewrite_depth` | `false` | Transparent modes: draw the object's depth first, then its colour only on its nearest surface, so a see-through body shows no inner faces or overlapping parts: a clean ghost. Needs `depth_test`. Costs one more draw of the object. |

Pipelines for each combination are made once, when a material first uses it, and shared by every material with the same state.

## Material shaders

A `.material.json` with a `"shader"` key is a `ShaderMaterial`: your `.wgsl` file defines `fn surface(in: SurfaceInput, p: Params) -> Surface` (and optionally `fn vertex`), and its `@material struct` header declares the parameters the inspector shows. See `crates/box_engine/examples/assets/stripes.wgsl` for a commented example. A shader material has the alpha modes and the draw order and depth fields above, plus:

| Field | Default | What it does |
|---|---|---|
| `params` | `[]` | Parameter values by name; missing ones use the header default. |
| `depth_fade` | `0` | Transparent modes: fade out over this many metres in front of the surface behind (soft edges). |
| `scene_depth` | `false` | The shader reads the scene depth: run the depth prepass while the material is visible. A shader that calls `scene_depth` turns this on by itself. |
| `bounds_padding` | `0` | Metres added on every side of the culling bounds of meshes drawn with this material. Set it to how far your `vertex` hook moves vertices (waves, sway), or the mesh can vanish at the edge of the view, or lose its shadow, while still on screen. |

### Reading the scene depth

A transparent material shader can ask how far the opaque scene is behind each pixel, for soft intersections, shorelines and depth tint:

- `scene_depth(in.frag_coord)`: the view depth, in metres along the camera's view axis, of the nearest opaque surface at this pixel.
- `fragment_view_depth(in.frag_coord)`: the view depth of the pixel being shaded.
- `scene_depth_available()`: whether the depth is there.

Their difference is how much space lies behind the surface. Where there is no depth (the sky, a frame without the prepass, the shadow passes), `scene_depth` returns `SCENE_DEPTH_FAR`, a billion metres, so the gap reads as "very deep" and nothing breaks. The scene depth holds opaque and masked surfaces only; an opaque material reads its own depth.

This shader draws water that is clear in the shallows, dark in the deep, with foam where it meets geometry and small waves from its `vertex` hook (`crates/box_engine/examples/assets/shoreline.wgsl`, drawn in `cargo run --example materials -- --scene pipeline_state`):

```wgsl
fn surface(in: SurfaceInput, p: Shoreline) -> Surface {
    var s = default_surface(in);
    // Metres of water between this pixel and what is behind it, along the view.
    let gap = max(scene_depth(in.frag_coord) - fragment_view_depth(in.frag_coord), 0.0);
    let t = saturate(gap / p.depth);
    s.base_color = mix(p.shallow, p.deep, t);
    s.alpha = mix(0.3, 0.9, t);
    s.roughness = 0.1;
    let foam = 1.0 - smoothstep(0.0, max(p.foam, 1e-4), gap);
    s.base_color = mix(s.base_color, vec3f(0.9, 0.95, 0.95), foam);
    s.alpha = max(s.alpha, foam);
    return s;
}
```

Its material file uses `"Blend"` and pads its bounds by the wave height:

```json
{
  "shader": "shaders/shoreline.wgsl",
  "alpha_mode": "Blend",
  "bounds_padding": 0.05
}
```

### Custom vertex data

A mesh can carry up to two sets of four free numbers per vertex for your shaders: growth stamps, flow directions, wetness, anything a generator knows and a shader needs. Fill `MeshData::custom` (one `Vec<Vec4>` per set), or in a `MeshBuilder` call `with_custom_sets(n)` and set `Vertex::custom[0]` / `custom[1]`. The engine uploads them unchanged and never reads them.

- In `surface`, `in.custom0` and `in.custom1` are the values interpolated across the triangle.
- In the `vertex` hook, `in.custom0` and `in.custom1` are the vertex's own values.

A mesh without custom data reads zero in both. This shader colours a surface from set 0 and grows it out of the ground by a per-vertex delay in set 1:

```wgsl
fn surface(in: SurfaceInput, p: Sprout) -> Surface {
    var s = default_surface(in);
    s.base_color = in.custom0.rgb;
    return s;
}

fn vertex(position: vec3f, in: VertexInput, p: Sprout) -> vec3f {
    // custom1.x: seconds before this vertex starts to rise; custom1.y: how far.
    let grown = saturate((globals.time.x - in.custom1.x) / p.duration);
    return position - vec3f(0.0, in.custom1.y * (1.0 - grown), 0.0);
}
```

Every vertex is 32 bytes larger for the two sets, whether a mesh uses them or not. Imported glTF models have no custom data, and Bake to Mesh does not write it.

### Reading the scene colour

A transparent material shader can also read the colour of the opaque scene behind it, for refraction, heat haze and frosted glass:

- `scene_color(in.frag_coord)`: the colour straight behind this pixel.
- `scene_uv(in.frag_coord)`: this pixel's position in the view, 0..1 from the top left.
- `scene_color_at(uv)`: the filtered colour at a view position (clamped at the edges). Add an offset to `scene_uv` to bend the view.
- `scene_depth_at(uv)`: the scene depth at a view position, to check what an offset sample landed on.
- `scene_color_available()`: whether the colour is there.

The colour is linear HDR: lit and fogged, before exposure and tone mapping. It holds opaque and masked surfaces, the sky and scene lines, never other transparent surfaces. Calling `scene_color` or `scene_color_at` is all it takes: while such a material is visible, the engine copies the opaque scene before it draws transparent surfaces (about 0.1 ms at 2560x1440 on an Apple M-series GPU). Opaque and masked materials have no colour to read and get black.

To show the scene through the surface, return alpha 1 and put the colour you read into `emissive` (it is already lit); `base_color` stays the surface's own, lit colour:

```wgsl
fn surface(in: SurfaceInput, p: Glass) -> Surface {
    var s = default_surface(in);
    let here = fragment_view_depth(in.frag_coord);
    var uv = scene_uv(in.frag_coord) + in.normal.xz * p.bend;
    if scene_depth_at(uv) < here {
        // The bent view landed on something in front of the surface.
        uv = scene_uv(in.frag_coord);
    }
    s.emissive = scene_color_at(uv) * p.tint;
    s.base_color = vec3f(0.0);
    s.alpha = 1.0;
    return s;
}
```

`crates/box_engine/examples/assets/pond.wgsl` is refracting water built this way: ripples bend the view of the bottom, the water tints and absorbs the light that crosses it, and foam marks where it meets geometry (`cargo run --example materials -- --scene refraction`). Limits: an offset sample cannot see what is outside the view or hidden behind nearer opaque geometry; with MSAA the copy is the resolved colour, so a bent sample right at a silhouette can pick up a thin edge of the object in front; the engine fogs the surface's result, the refracted colour included (which was fogged once already), so far-away refracting surfaces in thick fog read slightly too foggy.
