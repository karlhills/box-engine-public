//! Every magic offset of box_arch in one place (spike S6: "magic offsets
//! belong in one place as named, documented constants"). Metres unless
//! noted. Defaults follow stylized, slightly chunky real-world proportions.

/// Corners whose mitre would stretch a wall's section by more than this
/// factor (interior angle below about 29°) are split: both sides end
/// square and a bevel fill closes the outside notch.
pub const MITRE_LIMIT: f32 = 4.0;
/// Interior joints turning less than this (radians) shade smooth: arcs,
/// sampled splines.
pub const SMOOTH_JOINT: f32 = 0.2;
/// Arcs are cut into chords turning at most this much (radians, 10°).
pub const ARC_STEP: f32 = std::f32::consts::PI / 18.0;
/// Points per round arch top (half circle) or per side of a pointed one.
pub const ARCH_SEGMENTS: usize = 10;
/// Spacing of top samples along a face when the roof underside is curved
/// (cone, dome).
pub const TOP_SAMPLE_STEP: f32 = 0.25;
/// Lowest top a wall may have (a roof underside below this is clamped).
pub const MIN_WALL_TOP: f32 = 0.05;

/// Plinth band: height and how far it stands proud of the faces.
pub const PLINTH_HEIGHT: f32 = 0.32;
pub const PLINTH_PROUD: f32 = 0.035;
/// Plinths stop this far short of door jambs, so a plinth end never lies
/// in a reveal's plane.
pub const PLINTH_DOOR_GAP: f32 = 0.05;
/// Cornice band under the visible top (outer side only).
pub const CORNICE_HEIGHT: f32 = 0.14;
pub const CORNICE_PROUD: f32 = 0.05;
/// Coping over a free-standing wall's flat top: from this far below the top
/// to this far above it, proud on both sides.
pub const COPING_DROP: f32 = 0.1;
pub const COPING_RISE: f32 = 0.06;
pub const COPING_PROUD: f32 = 0.045;

/// Window sill: height, projection and overrun past the jambs.
pub const SILL_HEIGHT: f32 = 0.07;
pub const SILL_PROUD: f32 = 0.06;
pub const SILL_OVERRUN: f32 = 0.06;
/// Door threshold: height, projection and overrun past the jambs.
pub const THRESHOLD_HEIGHT: f32 = 0.05;
pub const THRESHOLD_PROUD: f32 = 0.12;
pub const THRESHOLD_OVERRUN: f32 = 0.05;
/// Doors and passages that start at the wall foot are cut this far below
/// it, so the face's bottom edge is removed cleanly.
pub const DOOR_SINK: f32 = 0.05;
/// Panel set-back from the outer face, as a fraction of the thickness.
pub const WINDOW_SETBACK: f32 = 0.33;
pub const DOOR_SETBACK: f32 = 0.42;
/// Glazing bars: width and depth.
pub const GLAZING_BAR_WIDTH: f32 = 0.045;
pub const GLAZING_BAR_DEPTH: f32 = 0.05;
/// The horizontal glazing bar sits at this fraction of the straight part.
pub const GLAZING_BAR_RISE: f32 = 0.55;

/// Opening clearances (spike S6, rule J7): from hard corners and ends
/// (plus the corner's reach), between openings, and below the visible top.
pub const OPENING_CORNER_MARGIN: f32 = 0.25;
pub const OPENING_GAP: f32 = 0.3;
pub const OPENING_TOP_MARGIN: f32 = 0.25;
/// Smallest opening width and height.
pub const MIN_OPENING: f32 = 0.1;
/// An arch needs at least this much straight jamb under its springing.
pub const MIN_JAMB: f32 = 0.05;

/// Ambient occlusion (factors before the generator's strength):
/// darkening at the wall foot over this height...
pub const BASE_AO_HEIGHT: f32 = 0.65;
pub const BASE_AO: f32 = 0.42;
/// ...under a roof's eaves over this depth below the top...
pub const EAVE_AO_DEPTH: f32 = 0.75;
pub const EAVE_AO: f32 = 0.38;
/// ...in concave corners (exponential falloff along the wall; faces are
/// sliced this far either side of the corner)...
pub const CORNER_AO: f32 = 0.3;
pub const CORNER_AO_FALLOFF: f32 = 0.28;
pub const CORNER_AO_BREAK: f32 = 0.6;
/// Occlusion slice lines this close to an opening edge or a joint move onto
/// it, so slicing never leaves sub-millimetre slivers.
pub const BREAK_SNAP: f32 = 0.005;
/// ...and at the panel line deep in a reveal.
pub const REVEAL_AO: f32 = 0.6;

/// How far a roof end runs into the wall it abuts (spike S6, rule J9).
pub const ABUT_PENETRATION: f32 = 0.03;
/// Fascia and barge boards: thickness, drop below the eave line and rise
/// above the tile surface.
pub const BOARD_THICKNESS: f32 = 0.045;
pub const BOARD_BELOW: f32 = 0.035;
pub const BOARD_ABOVE: f32 = 0.035;
/// Ridge and hip rolls: width, height and how far they sink into the tiles.
pub const RIDGE_WIDTH: f32 = 0.2;
pub const RIDGE_HEIGHT: f32 = 0.07;
pub const RIDGE_SINK: f32 = 0.03;
/// Soffit occlusion at the wall line (rising to 0.9 at the eave).
pub const SOFFIT_AO: f32 = 0.5;

/// Stair limits: the steepest going (tread depth) and the tallest riser
/// a stair builds without a warning, and the shallowest going before a
/// warning that the run is too long for its rise.
pub const MIN_GOING: f32 = 0.2;
pub const MAX_GOING: f32 = 0.6;
pub const MAX_STEPS: u32 = 200;
/// Riser occlusion at the foot of each riser.
pub const RISER_AO: f32 = 0.55;
