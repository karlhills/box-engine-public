//! Golden fixtures for generators: output hashes pinned to `VERSION`
//! (docs/PROCEDURAL.md section 6).
//!
//! Each generator type has one fixture file, `<dir>/<Type>.golden.json`:
//!
//! ```json
//! {
//!   "generator": "Steps",
//!   "version": 1,
//!   "cases": [
//!     { "name": "default", "seed": 0, "params": { ... },
//!       "hash": "0x1f2e...", "vertices": 96, "triangles": 48, "warnings": [] }
//!   ]
//! }
//! ```
//!
//! [`assert_golden`] regenerates every case from its **stored** parameters
//! and seed and compares [`mesh_hash`](crate::mesh_hash) (quantized, so
//! sub-0.1 mm float noise passes):
//!
//! - same `VERSION`, different hash: **fails**, also when re-recording.
//!   Output changed without a version bump, so saved worlds would silently
//!   change. Bump `VERSION`, then re-record.
//! - `VERSION` differs from the fixture: fails until re-recorded with
//!   `BOX_UPDATE_GOLDEN=1` (then review the generator's golden captures).
//! - a case in code that the fixture lacks, or whose parameters or seed
//!   changed in code: fails until re-recorded.
//! - no fixture: fails until recorded.
//!
//! Cases only in the fixture are still checked and kept.
//!
//! **Inputs.** A generator that reads other entities' components (a
//! `Sweep` reads a `Spline`) gets them from the case:
//! [`GoldenCase::with_owner`] and [`GoldenCase::with_input`]. They are
//! stored in the case as `"owner"` and `"inputs": [{"uid", "component",
//! "value"}]` (both omitted when unused, so older fixtures stay valid) and
//! compared like the parameters. A fixture-only case can use only input
//! component types that some case in code also uses.

use std::any::{Any, TypeId};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy_reflect::{FromReflect, GetTypeRegistration, Reflect, TypePath};
use box_core::identity::Uid;
use box_core::reflect::Registry;
use box_core::world::Component;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::generator::{GenContext, Generator, GeneratorRegistry, run_generate};
use crate::hash::mesh_hash;
use crate::inputs::{InputId, InputValue, ResolvedInput};

/// The environment variable that re-records fixtures (shared with the
/// renderer's golden images).
pub const UPDATE_ENV: &str = "BOX_UPDATE_GOLDEN";

/// One parameter set of a generator, as written in a test.
///
/// A generator that reads inputs gets them from the case: the owner's
/// `Uid` ([`GenContext::owner`](crate::GenContext::owner)) and component
/// values of entities by `Uid` ([`GenContext::component`](crate::GenContext::component)):
///
/// ```ignore
/// let uid = Uid::from_u128(1);
/// GoldenCase::new("tube", Sweep::default(), 0)
///     .with_owner(uid)
///     .with_input(uid, Spline::default());
/// ```
///
/// They are stored in the fixture next to the parameters (`owner`,
/// `inputs: [{uid, component, value}]`) and part of what must match.
#[derive(Clone, Debug)]
pub struct GoldenCase<T> {
    pub name: String,
    pub params: T,
    pub seed: u64,
    /// The entity being generated (`None`: no owner).
    pub owner: Option<Uid>,
    /// Component values of entities the generator reads.
    pub inputs: Vec<GoldenInput>,
}

impl<T> GoldenCase<T> {
    pub fn new(name: impl Into<String>, params: T, seed: u64) -> Self {
        Self {
            name: name.into(),
            params,
            seed,
            owner: None,
            inputs: Vec::new(),
        }
    }

    /// Generate as the entity `uid`.
    pub fn with_owner(mut self, uid: Uid) -> Self {
        self.owner = Some(uid);
        self
    }

    /// The entity `uid` has the component `value` (replacing an earlier
    /// value of the same type for that entity).
    pub fn with_input<C>(mut self, uid: Uid, value: C) -> Self
    where
        C: Component + Reflect + FromReflect + TypePath + GetTypeRegistration,
    {
        self.inputs
            .retain(|i| !(i.uid == uid && i.type_id == TypeId::of::<C>()));
        self.inputs.push(GoldenInput {
            uid,
            type_id: TypeId::of::<C>(),
            type_name: C::short_type_path(),
            value: Arc::new(value),
            codec: InputCodec::of::<C>(),
        });
        self
    }
}

/// A component value a [`GoldenCase`] supplies as a generator input.
#[derive(Clone)]
pub struct GoldenInput {
    pub uid: Uid,
    type_id: TypeId,
    type_name: &'static str,
    value: Arc<dyn Any + Send + Sync>,
    codec: InputCodec,
}

impl std::fmt::Debug for GoldenInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoldenInput")
            .field("uid", &self.uid)
            .field("component", &self.type_name)
            .finish_non_exhaustive()
    }
}

impl GoldenInput {
    /// The component's short type name.
    pub fn component(&self) -> &'static str {
        self.type_name
    }
}

type ToJson = fn(&Registry, &(dyn Any + Send + Sync)) -> Result<Json, String>;
type FromJson = fn(&Registry, &Json) -> Result<Arc<dyn Any + Send + Sync>, String>;

/// How an input type is registered, written and read back.
#[derive(Clone, Copy)]
struct InputCodec {
    type_id: TypeId,
    register: fn(&mut Registry),
    to_json: ToJson,
    from_json: FromJson,
}

impl InputCodec {
    fn of<C>() -> Self
    where
        C: Component + Reflect + FromReflect + TypePath + GetTypeRegistration,
    {
        Self {
            type_id: TypeId::of::<C>(),
            register: |r| {
                let _ = r.register::<C>();
            },
            to_json: |r, v| {
                let v = v.downcast_ref::<C>().ok_or("input of the wrong type")?;
                r.value_to_json(v).map_err(|e| e.to_string())
            },
            from_json: |r, json| {
                let v = r
                    .value_from_json(TypeId::of::<C>(), json)
                    .map_err(|e| e.to_string())?;
                let c = C::from_reflect(v.as_ref()).ok_or("shape mismatch")?;
                Ok(Arc::new(c))
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Fixture {
    generator: String,
    version: u32,
    cases: Vec<FixtureCase>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FixtureCase {
    name: String,
    seed: u64,
    params: Json,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<Uid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    inputs: Vec<FixtureInput>,
    hash: String,
    vertices: usize,
    triangles: usize,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FixtureInput {
    uid: Uid,
    component: String,
    value: Json,
}

impl FixtureCase {
    /// Same parameters, seed, owner and inputs (what a case in code is
    /// compared with).
    fn same_setup(&self, other: &FixtureCase) -> bool {
        self.params == other.params
            && self.seed == other.seed
            && self.owner == other.owner
            && self.inputs == other.inputs
    }
}

/// Why a golden check failed. Messages say what to do.
#[derive(Debug, thiserror::Error)]
pub enum GoldenError {
    #[error(
        "no golden fixture for `{generator}` at {path}; record it with {UPDATE_ENV}=1 and review the generator's captures"
    )]
    Missing { generator: String, path: PathBuf },
    #[error(
        "`{generator}` output changed for golden case `{case}` (hash {recorded} -> {actual}) but {generator}::VERSION is still {version}; saved worlds would silently change. Bump VERSION, then re-record with {UPDATE_ENV}=1 and review the captures"
    )]
    Unversioned {
        generator: String,
        case: String,
        version: u32,
        recorded: String,
        actual: String,
    },
    #[error(
        "{generator}::VERSION is {version} but {path} was recorded at version {recorded}; re-record with {UPDATE_ENV}=1 and review the captures"
    )]
    VersionChanged {
        generator: String,
        path: PathBuf,
        version: u32,
        recorded: u32,
    },
    #[error(
        "golden case `{case}` of `{generator}` is new or its parameters or seed changed in code; record it with {UPDATE_ENV}=1"
    )]
    Unrecorded { generator: String, case: String },
    #[error("golden case `{case}`: stored parameters do not load as `{generator}`: {message}")]
    BadParams {
        generator: String,
        case: String,
        message: String,
    },
    #[error("{path}: {message}")]
    File { path: PathBuf, message: String },
}

/// What a passing check did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoldenReport {
    pub path: PathBuf,
    /// The fixture was (re)written.
    pub recorded: bool,
    /// Cases checked or recorded.
    pub cases: usize,
}

/// The fixture file of `T` in `dir`.
pub fn fixture_path<T: Generator>(dir: &Path) -> PathBuf {
    dir.join(format!("{}.golden.json", T::short_type_path()))
}

/// [`check_golden`], re-recording when `BOX_UPDATE_GOLDEN` is set; panics
/// with the error's instructions on failure.
pub fn assert_golden<T: Generator>(dir: impl AsRef<Path>, cases: &[GoldenCase<T>]) {
    let update = std::env::var_os(UPDATE_ENV).is_some();
    if let Err(e) = check_golden(dir.as_ref(), cases, update) {
        panic!("{e}");
    }
}

/// Checks (or with `update`, records) `T`'s fixture in `dir` against
/// `cases`. See the [module docs](self) for the rules; an output change
/// without a `VERSION` bump fails even with `update`.
pub fn check_golden<T: Generator>(
    dir: &Path,
    cases: &[GoldenCase<T>],
    update: bool,
) -> Result<GoldenReport, GoldenError> {
    let generator = T::short_type_path().to_string();
    let path = fixture_path::<T>(dir);
    let mut registry = Registry::with_core_types();
    let _ = registry.register_generator::<T>();
    // Input types by name, from the cases in code (fixture-only cases may
    // use only these).
    let mut codecs: Vec<(&'static str, InputCodec)> = Vec::new();
    for i in cases.iter().flat_map(|c| &c.inputs) {
        if !codecs.iter().any(|(n, _)| *n == i.type_name) {
            (i.codec.register)(&mut registry);
            codecs.push((i.type_name, i.codec));
        }
    }
    let file_err = |message: String| GoldenError::File {
        path: path.clone(),
        message,
    };

    let fresh: Vec<FixtureCase> = cases
        .iter()
        .map(|c| {
            let params = registry
                .value_to_json(&c.params)
                .map(as_stored)
                .map_err(|e| file_err(e.to_string()))?;
            let inputs = c
                .inputs
                .iter()
                .map(|i| {
                    Ok(FixtureInput {
                        uid: i.uid,
                        component: i.type_name.to_string(),
                        value: (i.codec.to_json)(&registry, i.value.as_ref())
                            .map(as_stored)
                            .map_err(file_err)?,
                    })
                })
                .collect::<Result<Vec<_>, GoldenError>>()?;
            Ok(record(
                &c.name,
                c.seed,
                params,
                (c.owner, inputs),
                &c.params,
                &c.inputs,
            ))
        })
        .collect::<Result<_, GoldenError>>()?;

    let existing: Option<Fixture> = match std::fs::read_to_string(&path) {
        Ok(text) => Some(serde_json::from_str(&text).map_err(|e| file_err(e.to_string()))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(file_err(e.to_string())),
    };

    let write = |fixture: &Fixture| -> Result<GoldenReport, GoldenError> {
        std::fs::create_dir_all(dir).map_err(|e| file_err(e.to_string()))?;
        let mut text =
            serde_json::to_string_pretty(fixture).map_err(|e| file_err(e.to_string()))?;
        text.push('\n');
        std::fs::write(&path, text).map_err(|e| file_err(e.to_string()))?;
        Ok(GoldenReport {
            path: path.clone(),
            recorded: true,
            cases: fixture.cases.len(),
        })
    };

    let Some(existing) = existing else {
        if !update {
            return Err(GoldenError::Missing { generator, path });
        }
        return write(&Fixture {
            generator,
            version: T::VERSION,
            cases: fresh,
        });
    };
    if existing.version != T::VERSION {
        if !update {
            return Err(GoldenError::VersionChanged {
                generator,
                path,
                version: T::VERSION,
                recorded: existing.version,
            });
        }
        return write(&Fixture {
            generator,
            version: T::VERSION,
            cases: fresh,
        });
    }

    // Same version: every stored case must still produce its hash.
    for case in &existing.cases {
        let bad = |message: String| GoldenError::BadParams {
            generator: generator.clone(),
            case: case.name.clone(),
            message,
        };
        let value = registry
            .value_from_json(TypeId::of::<T>(), &case.params)
            .map_err(|e| e.to_string())
            .and_then(|v| T::from_reflect(v.as_ref()).ok_or_else(|| "shape mismatch".to_string()))
            .map_err(bad)?;
        let inputs = case
            .inputs
            .iter()
            .map(|i| {
                let (name, codec) = codecs
                    .iter()
                    .find(|(n, _)| *n == i.component)
                    .ok_or_else(|| {
                        bad(format!(
                            "input component `{}` is not used by any case in code, so it cannot be read",
                            i.component
                        ))
                    })?;
                Ok(GoldenInput {
                    uid: i.uid,
                    type_id: codec.type_id,
                    type_name: name,
                    value: (codec.from_json)(&registry, &i.value).map_err(bad)?,
                    codec: *codec,
                })
            })
            .collect::<Result<Vec<_>, GoldenError>>()?;
        let now = record(
            &case.name,
            case.seed,
            case.params.clone(),
            (case.owner, case.inputs.clone()),
            &value,
            &inputs,
        );
        if now.hash != case.hash {
            return Err(GoldenError::Unversioned {
                generator,
                case: case.name.clone(),
                version: T::VERSION,
                recorded: case.hash.clone(),
                actual: now.hash,
            });
        }
    }
    let mut merged = existing.clone();
    for f in &fresh {
        match merged.cases.iter_mut().find(|c| c.name == f.name) {
            Some(c) if c.same_setup(f) => {}
            Some(c) => {
                if !update {
                    return Err(GoldenError::Unrecorded {
                        generator,
                        case: f.name.clone(),
                    });
                }
                *c = f.clone();
            }
            None => {
                if !update {
                    return Err(GoldenError::Unrecorded {
                        generator,
                        case: f.name.clone(),
                    });
                }
                merged.cases.push(f.clone());
            }
        }
    }
    if merged != existing {
        return write(&merged);
    }
    Ok(GoldenReport {
        path,
        recorded: false,
        cases: existing.cases.len(),
    })
}

/// `json` as it reads back from the fixture file: serde_json's default
/// float parser does not always round-trip 17-digit numbers exactly, so a
/// fresh value is compared with a stored one only after the same trip.
fn as_stored(json: Json) -> Json {
    serde_json::to_string(&json)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(json)
}

/// Runs `generator` with the case's owner and inputs and records the output.
fn record<T: Generator>(
    name: &str,
    seed: u64,
    params: Json,
    (owner, stored): (Option<Uid>, Vec<FixtureInput>),
    generator: &T,
    inputs: &[GoldenInput],
) -> FixtureCase {
    let resolved: Vec<ResolvedInput<'_>> = inputs
        .iter()
        .map(|i| ResolvedInput {
            id: InputId::Component {
                uid: i.uid,
                type_id: i.type_id,
                type_name: i.type_name,
            },
            value: Some(InputValue::Borrowed(i.value.as_ref() as &dyn Any)),
        })
        .collect();
    let out = run_generate(generator, &GenContext::with_inputs(seed, owner, &resolved));
    let (hash, vertices, triangles) = match &out.mesh {
        Some(m) => (mesh_hash(m), m.vertex_count(), m.triangle_count()),
        None => (0, 0, 0),
    };
    FixtureCase {
        name: name.to_string(),
        seed,
        params,
        owner,
        inputs: stored,
        hash: format!("{hash:#018x}"),
        vertices,
        triangles,
        warnings: out.warnings,
    }
}
