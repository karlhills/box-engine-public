-- box-engine script API: luau-lsp definitions, generated from the type registry.
-- Do not edit; regenerate with `box script types --project <dir>`.

-- Components
--- Replaces the sky ambient light inside a box, so interiors get no sky ambient leak. The box is the entity's transform (centre, rotation, scale) times `half_extents`.
export type AmbientVolume = { half_extents: vector, color: Color, intensity: number, sky_contribution: number, blend_distance: number }
--- The fields of `AmbientVolume` to change, each optional (what `set` takes).
export type AmbientVolumePatch = { half_extents: vector?, color: Color?, intensity: number?, sky_contribution: number?, blend_distance: number? }
--- Plays an `AnimationClip` on this entity and its descendants: each channel moves the entity whose `Name` matches its target (the joints of an instanced glTF character). Changing `clip` crossfades from the clip playing to the new one over `crossfade` seconds. Saved in scenes; the playback state is runtime only (`AnimationPlayerState`). An `AnimationStateMachine` on the same entity drives these fields.
export type AnimationPlayer = { clip: string?, speed: number, looping: boolean, paused: boolean, crossfade: number }
--- The fields of `AnimationPlayer` to change, each optional (what `set` takes).
export type AnimationPlayerPatch = { clip: string?, speed: number?, looping: boolean?, paused: boolean?, crossfade: number? }
--- Runs an `AnimationGraph` on this entity's `AnimationPlayer` (added with defaults when missing): entering a state sets the player's clip, speed and looping, crossfading by the transition's duration. Game code (and scripts) steer it through parameters: `set_float`, `set_bool`, `trigger`. Saved in scenes (parameters as their initial values); the current state is runtime only (`AnimationStateMachineState`).
export type AnimationStateMachine = { source: string?, graph: { states: { { name: string, clip: string?, speed: number, looping: boolean } }, transitions: { { from: string, to: string, conditions: { { parameter: string, test: { Greater: number } | { Less: number } | "IsTrue" | "IsFalse" | "Triggered" } }, duration: number, exit_time: number } }, start: string }, parameters: { { name: string, value: { Float: number } | { Bool: boolean } | { Trigger: boolean } } } }
--- The fields of `AnimationStateMachine` to change, each optional (what `set` takes).
export type AnimationStateMachinePatch = { source: string?, graph: ({ states: { { name: string, clip: string?, speed: number, looping: boolean } }, transitions: { { from: string, to: string, conditions: { { parameter: string, test: { Greater: number } | { Less: number } | "IsTrue" | "IsFalse" | "Triggered" } }, duration: number, exit_time: number } }, start: string })?, parameters: ({ { name: string, value: { Float: number } | { Bool: boolean } | { Trigger: boolean } } })? }
--- Where sounds are heard from: positional sounds are panned and attenuated relative to the first entity with this component (its global position and rotation; -Z is forward, +X is right). Without one, the active camera listens.
export type AudioListener = {}
--- The fields of `AudioListener` to change, each optional (what `set` takes).
export type AudioListenerPatch = {}
--- A sound attached to an entity: an ambience loop, a music track, or a sound that plays once when the entity appears. It plays while the entity exists in a running game (the player, editor Play mode; never in the edit world), fades in when it starts and fades out when the entity is despawned (including by a scene switch) or `playing` is turned off.
export type AudioSource = { clip: string?, bus: "Effects" | "Ambience" | "Music", volume: number, pitch: number, looping: boolean, playing: boolean, fade_in: number, fade_out: number, spatial: boolean, min_distance: number, max_distance: number }
--- The fields of `AudioSource` to change, each optional (what `set` takes).
export type AudioSourcePatch = { clip: string?, bus: ("Effects" | "Ambience" | "Music")?, volume: number?, pitch: number?, looping: boolean?, playing: boolean?, fade_in: number?, fade_out: number?, spatial: boolean?, min_distance: number?, max_distance: number? }
--- Floats the entity up and down around where it started. Copies placed in different spots bob out of step.
export type Bobber = { enabled: boolean, height: number, period: number }
--- The fields of `Bobber` to change, each optional (what `set` takes).
export type BobberPatch = { enabled: boolean?, height: number?, period: number? }
--- A perspective camera. Uses the entity's `GlobalTransform` (looks down −Z). The projection is an infinite reverse-Z perspective; the aspect ratio comes from the render target.
export type Camera = { fov_y: number, near: number, active: boolean }
--- The fields of `Camera` to change, each optional (what `set` takes).
export type CameraPatch = { fov_y: number?, near: number?, active: boolean? }
--- A simple enemy: waits (or walks its `patrol` points) until it sees the player within `sight`, then chases them along the navmesh at `chase_speed`, around walls and up stairs. Touching the player costs them `damage` (their `Health`, or a respawn without one) at most once per `attack_cooldown`. When it loses sight of the player it searches where it saw them last; beyond `give_up` it walks back home. Uses a `NavAgent` and a `KinematicCharacter` (added in Play when missing).
export type Chaser = { enabled: boolean, speed: number, chase_speed: number, sight: number, give_up: number, line_of_sight: boolean, patrol: { vector }, wait: number, damage: number, attack_cooldown: number, reach: number }
--- The fields of `Chaser` to change, each optional (what `set` takes).
export type ChaserPatch = { enabled: boolean?, speed: number?, chase_speed: number?, sight: number?, give_up: number?, line_of_sight: boolean?, patrol: { vector }?, wait: number?, damage: number?, attack_cooldown: number?, reach: number? }
--- Whether this entity's colliders and those of its descendants take part in physics (default `true`). `Collidable(false)` switches the whole subtree off: queries, triggers, characters and rigid bodies ignore its colliders until it is set back to `true` (a hidden bridge, a door that opened). A parent's `false` wins over its children's `true`. Saved in scenes. The physics counterpart of `Visible`, which only hides drawing.
export type Collidable = boolean
--- The fields of `Collidable` to change, each optional (what `set` takes).
export type CollidablePatch = boolean
--- A collision shape on an entity, placed by its `GlobalTransform`. Saved in scenes. Queries on `Physics` find colliders; trigger colliders also report overlaps as `TriggerEvent`s.
export type Collider = { shape: "Primitive" | { Cuboid: { size: vector? } } | { Sphere: { radius: number? } } | { Capsule: { radius: number?, half_height: number? } } | { Cylinder: { radius: number?, height: number? } } | "Mesh" | "ConvexHull", offset: vector, is_trigger: boolean, layers: number, mask: number, friction: number, restitution: number }
--- The fields of `Collider` to change, each optional (what `set` takes).
export type ColliderPatch = { shape: ("Primitive" | { Cuboid: { size: vector? } } | { Sphere: { radius: number? } } | { Capsule: { radius: number?, half_height: number? } } | { Cylinder: { radius: number?, height: number? } } | "Mesh" | "ConvexHull")?, offset: vector?, is_trigger: boolean?, layers: number?, mask: number?, friction: number?, restitution: number? }
--- The primary directional light (the sun). Its direction is the entity's `GlobalTransform::forward()`. The renderer uses the first one it finds.
export type DirectionalLight = { color: Color, intensity: number, shadows: boolean, shadow_distance: number }
--- The fields of `DirectionalLight` to change, each optional (what `set` takes).
export type DirectionalLightPatch = { color: Color?, intensity: number?, shadows: boolean?, shadow_distance: number? }
--- A flat outline pushed up along the entity's Y axis into a solid (a slab, a plaque, a star, a frame), standing on the origin. Pick an outline and an optional hole; the bevel rounds or chamfers the top and bottom edges. Material slots: Sides, Caps (top and bottom), Bevel.
export type Extrude = { outline: { Rectangle: { size: vector?, corner_radius: number? } } | { Circle: { radius: number?, segments: number? } } | { Polygon: { sides: number?, radius: number? } } | { Star: { points: number?, outer_radius: number?, inner_radius: number? } } | { Custom: { points: { vector }? } }, hole: "None" | { Inset: { width: number? } } | { Circle: { radius: number?, segments: number?, offset: vector? } } | { Rectangle: { size: vector?, offset: vector? } } | { Custom: { rings: { { vector } }? } }, depth: number, bevel_width: number, bevel_segments: number, caps: boolean, smooth_angle: number }
--- The fields of `Extrude` to change, each optional (what `set` takes).
export type ExtrudePatch = { outline: ({ Rectangle: { size: vector?, corner_radius: number? } } | { Circle: { radius: number?, segments: number? } } | { Polygon: { sides: number?, radius: number? } } | { Star: { points: number?, outer_radius: number?, inner_radius: number? } } | { Custom: { points: { vector }? } })?, hole: ("None" | { Inset: { width: number? } } | { Circle: { radius: number?, segments: number?, offset: vector? } } | { Rectangle: { size: vector?, offset: vector? } } | { Custom: { rings: { { vector } }? } })?, depth: number?, bevel_width: number?, bevel_segments: number?, caps: boolean?, smooth_angle: number? }
--- A camera that follows a target from behind and above. Put it on the entity with the `Camera`. In Play it becomes the active camera (other cameras are switched off), so the scene's starting camera need not be removed.
export type FollowCamera = { enabled: boolean, target: string?, mode: "Orbit" | "Fixed", distance: number, height: number, pitch: number, smoothing: number, look_speed: number, mouse_sensitivity: number, capture_mouse: boolean }
--- The fields of `FollowCamera` to change, each optional (what `set` takes).
export type FollowCameraPatch = { enabled: boolean?, target: string?, mode: ("Orbit" | "Fixed")?, distance: number?, height: number?, pitch: number?, smoothing: number?, look_speed: number?, mouse_sensitivity: number?, capture_mouse: boolean? }
--- Keeps an entity on the ground: its origin's height is the ground under it plus `offset`, recomputed when the ground, the entity's position on X and Z, or its parent moves. Move it freely on X and Z; its own height is owned by the ground. Not for parts made by a rule (a rule samples the ground itself).
export type FollowGround = { offset: number }
--- The fields of `FollowGround` to change, each optional (what `set` takes).
export type FollowGroundPatch = { offset: number? }
--- Hit points. Hits (the `Damage` action) lower `current`, then nothing can hurt it for `invulnerable` seconds; `Heal` raises it up to `max`. When it reaches 0, `on_zero` runs. Left empty, `on_zero` sends the player back to the last checkpoint (costing a life when the rules count lives, and refilling its health), and removes anything else (hides it, or despawns a spawned copy). Saved: `current` is its state.
export type Health = { enabled: boolean, max: number, current: number, invulnerable: number, on_damage: { { Show: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Hide: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Toggle: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { StartParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { StopParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { SetLight: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, intensity: number? } } | { AddVar: { var: string?, amount: number? } } | { SetVar: { var: string?, value: number? } } | { SetEnabled: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, enabled: boolean? } } | { PlaySound: { clip: string?, volume: number? } } | { Signal: { name: string? } } | { CallScript: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, ["function"]: string? } } | { SetField: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, path: string?, value: string? } } | "Win" | "Lose" | "Respawn" | "SetCheckpoint" | { SwitchScene: { scene: string? } } | { Burst: { style: ("Sparkle" | "Dust" | "Flame")?, count: number? } } | { Damage: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Heal: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Despawn: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } }, on_zero: { { Show: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Hide: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Toggle: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { StartParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { StopParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { SetLight: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, intensity: number? } } | { AddVar: { var: string?, amount: number? } } | { SetVar: { var: string?, value: number? } } | { SetEnabled: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, enabled: boolean? } } | { PlaySound: { clip: string?, volume: number? } } | { Signal: { name: string? } } | { CallScript: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, ["function"]: string? } } | { SetField: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, path: string?, value: string? } } | "Win" | "Lose" | "Respawn" | "SetCheckpoint" | { SwitchScene: { scene: string? } } | { Burst: { style: ("Sparkle" | "Dust" | "Flame")?, count: number? } } | { Damage: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Heal: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Despawn: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } } }
--- The fields of `Health` to change, each optional (what `set` takes).
export type HealthPatch = { enabled: boolean?, max: number?, current: number?, invulnerable: number?, on_damage: ({ { Show: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Hide: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Toggle: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { StartParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { StopParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { SetLight: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, intensity: number? } } | { AddVar: { var: string?, amount: number? } } | { SetVar: { var: string?, value: number? } } | { SetEnabled: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, enabled: boolean? } } | { PlaySound: { clip: string?, volume: number? } } | { Signal: { name: string? } } | { CallScript: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, ["function"]: string? } } | { SetField: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, path: string?, value: string? } } | "Win" | "Lose" | "Respawn" | "SetCheckpoint" | { SwitchScene: { scene: string? } } | { Burst: { style: ("Sparkle" | "Dust" | "Flame")?, count: number? } } | { Damage: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Heal: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Despawn: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } })?, on_zero: ({ { Show: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Hide: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Toggle: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { StartParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { StopParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { SetLight: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, intensity: number? } } | { AddVar: { var: string?, amount: number? } } | { SetVar: { var: string?, value: number? } } | { SetEnabled: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, enabled: boolean? } } | { PlaySound: { clip: string?, volume: number? } } | { Signal: { name: string? } } | { CallScript: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, ["function"]: string? } } | { SetField: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, path: string?, value: string? } } | "Win" | "Lose" | "Respawn" | "SetCheckpoint" | { SwitchScene: { scene: string? } } | { Burst: { style: ("Sparkle" | "Dust" | "Flame")?, count: number? } } | { Damage: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Heal: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Despawn: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } })? }
--- Something the player can use: press `action` within `radius` (or walk into it, with `trigger: Touch`); if every condition in `requires` holds, the `actions` run in order. Put it on the entity the player uses, usually a `PrefabInstance` root, so each placed copy has its own settings (its own `Entity` targets). Saved; `uses` is its state.
export type Interactable = { enabled: boolean, trigger: "Press" | "Touch", action: string, radius: number, offset: vector, prompt: string, prompt_key: string, blocked_prompt: string, blocked_prompt_key: string, requires: { { AtLeast: { var: string?, value: number? } } | { Below: { var: string?, value: number? } } | { Equals: { var: string?, value: number? } } | { GroupDone: { group: string? } } | { TimeAbove: { seconds: number? } } }, actions: { { Show: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Hide: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Toggle: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { StartParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { StopParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { SetLight: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, intensity: number? } } | { AddVar: { var: string?, amount: number? } } | { SetVar: { var: string?, value: number? } } | { SetEnabled: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, enabled: boolean? } } | { PlaySound: { clip: string?, volume: number? } } | { Signal: { name: string? } } | { CallScript: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, ["function"]: string? } } | { SetField: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, path: string?, value: string? } } | "Win" | "Lose" | "Respawn" | "SetCheckpoint" | { SwitchScene: { scene: string? } } | { Burst: { style: ("Sparkle" | "Dust" | "Flame")?, count: number? } } | { Damage: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Heal: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Despawn: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } }, once: boolean, cooldown: number, uses: number, group: string }
--- The fields of `Interactable` to change, each optional (what `set` takes).
export type InteractablePatch = { enabled: boolean?, trigger: ("Press" | "Touch")?, action: string?, radius: number?, offset: vector?, prompt: string?, prompt_key: string?, blocked_prompt: string?, blocked_prompt_key: string?, requires: ({ { AtLeast: { var: string?, value: number? } } | { Below: { var: string?, value: number? } } | { Equals: { var: string?, value: number? } } | { GroupDone: { group: string? } } | { TimeAbove: { seconds: number? } } })?, actions: ({ { Show: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Hide: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { Toggle: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, collide: boolean? } } | { StartParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { StopParticles: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } | { SetLight: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, intensity: number? } } | { AddVar: { var: string?, amount: number? } } | { SetVar: { var: string?, value: number? } } | { SetEnabled: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, enabled: boolean? } } | { PlaySound: { clip: string?, volume: number? } } | { Signal: { name: string? } } | { CallScript: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, ["function"]: string? } } | { SetField: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, path: string?, value: string? } } | "Win" | "Lose" | "Respawn" | "SetCheckpoint" | { SwitchScene: { scene: string? } } | { Burst: { style: ("Sparkle" | "Dust" | "Flame")?, count: number? } } | { Damage: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Heal: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")?, amount: number? } } | { Despawn: { target: ("This" | { Child: string } | { Entity: string? } | "Interactor")? } } })?, once: boolean?, cooldown: number?, uses: number?, group: string? }
--- Marks the player: the entity whose distance to interactables counts. Without one, the first `KinematicCharacter` is the interactor.
export type Interactor = {}
--- The fields of `Interactor` to change, each optional (what `set` takes).
export type InteractorPatch = {}
--- Draw this entity between its last two fixed-step transforms. For entities moved in `FixedUpdate` (characters, projectiles, moving platforms); see the module docs. Saved in scenes.
export type Interpolated = { teleport_distance: number }
--- The fields of `Interpolated` to change, each optional (what `set` takes).
export type InterpolatedPatch = { teleport_distance: number? }
--- Connects this entity's `RigidBody` to another body, or to the world. Saved in scenes. One joint per entity: build a chain by giving each link a joint to the previous one.
export type Joint = { kind: "Fixed" | "Hinge" | "Ball" | "Prismatic", connected: string?, anchor: vector, axis: vector, limits_enabled: boolean, limits: vector, collide_connected: boolean }
--- The fields of `Joint` to change, each optional (what `set` takes).
export type JointPatch = { kind: ("Fixed" | "Hinge" | "Ball" | "Prismatic")?, connected: string?, anchor: vector?, axis: vector?, limits_enabled: boolean?, limits: vector?, collide_connected: boolean? }
--- A capsule character moved by `CharacterPlugin` every fixed step: set the horizontal part of `velocity` (and call `jump`) from input in `FixedUpdate` before `move_characters`; the system adds gravity, moves with `Physics::move_and_slide` and updates `grounded`. The entity's origin is the capsule centre and its `Transform` is taken as world space (make it a root entity). The capsule stays upright whatever the entity's rotation.
export type KinematicCharacter = { radius: number, height: number, gravity: number, max_fall_speed: number, jump_speed: number, max_slope: number, ground_snap: number, skin: number, step_height: number, mass: number }
--- The fields of `KinematicCharacter` to change, each optional (what `set` takes).
export type KinematicCharacterPatch = { radius: number?, height: number?, gravity: number?, max_fall_speed: number?, jump_speed: number?, max_slope: number?, ground_snap: number?, skin: number?, step_height: number?, mass: number? }
--- A surface of revolution: a profile of (radius, height) points turned around the entity's Y axis. Pick a preset shape, or Custom for your own profile. Material slots: Surface, and Cap for the ends of a partial turn.
export type Lathe = { shape: { Vase: { height: number?, radius: number? } } | { Column: { height: number?, radius: number? } } | { Dome: { radius: number?, rings: number? } } | { Custom: { profile: { vector }?, closed: boolean? } }, segments: number, start_angle: number, sweep_angle: number, caps: boolean, smooth_angle: number }
--- The fields of `Lathe` to change, each optional (what `set` takes).
export type LathePatch = { shape: ({ Vase: { height: number?, radius: number? } } | { Column: { height: number?, radius: number? } } | { Dome: { radius: number?, rings: number? } } | { Custom: { profile: { vector }?, closed: boolean? } })?, segments: number?, start_angle: number?, sweep_angle: number?, caps: boolean?, smooth_angle: number? }
--- Draws this entity's `Primitive` with a material asset (a `.material.json` file, standard or shader, or a glTF material) instead of its inline material, following edits to the file (hot reload).
export type MaterialRef = { material: string? }
--- The fields of `MaterialRef` to change, each optional (what `set` takes).
export type MaterialRefPatch = { material: string? }
--- Draws the mesh asset `mesh` with `materials`, one per material group of the mesh (`MeshData::groups`, by slot). An empty list uses the materials an imported mesh (`model.glb#Mesh/<name>`) was imported with, in slot order (other meshes: the default material). A missing or empty material slot in a non-empty list uses the default material; a mesh that is not loaded yet draws nothing until it is, and a mesh that failed to load draws a placeholder.
export type MeshAsset = { mesh: string?, materials: { string? }, cast_shadows: boolean }
--- The fields of `MeshAsset` to change, each optional (what `set` takes).
export type MeshAssetPatch = { mesh: string?, materials: { string? }?, cast_shadows: boolean? }
--- Moves the entity through a list of points, measured from where it starts, and carries characters standing on it (moving platforms). Runs at the fixed rate before characters move.
export type Mover = { enabled: boolean, points: { vector }, speed: number, mode: "PingPong" | "Loop" | "Once", wait: number }
--- The fields of `Mover` to change, each optional (what `set` takes).
export type MoverPatch = { enabled: boolean?, points: { vector }?, speed: number?, mode: ("PingPong" | "Loop" | "Once")?, wait: number? }
--- Display name, shown in the hierarchy. Scene files write it as the entity's `"name"` key. Renaming is a field edit of `Name`.
export type Name = string
--- The fields of `Name` to change, each optional (what `set` takes).
export type NamePatch = string
--- Walks the entity's `KinematicCharacter` to `destination` along the navmesh, around walls and up ramps and stairs, turning the entity to face where it goes. Set `destination` from a script (`e:move_to(point)`), an action (`SetField`) or another behaviour (a `Chaser`); clear it to stop. A `KinematicCharacter` is added in Play when missing.
export type NavAgent = { enabled: boolean, speed: number, turn_rate: number, arrive_distance: number, destination: vector? }
--- The fields of `NavAgent` to change, each optional (what `set` takes).
export type NavAgentPatch = { enabled: boolean?, speed: number?, turn_rate: number?, arrive_distance: number?, destination: vector? }
--- Saved on the owner of derived parts: per-part field values and hidden parts that replace what the rules decided. Edit it through the inspector of a part (Cmd+Alt+click it in the viewport), or as data.
export type Overrides = { parts: { { rule: string, part: string, hidden: boolean, fields: { { path: string, value: string } } } } }
--- The fields of `Overrides` to change, each optional (what `set` takes).
export type OverridesPatch = { parts: { { rule: string, part: string, hidden: boolean, fields: { { path: string, value: string } } } }? }
--- Emits camera-facing particles from the entity's position, simulated on the CPU and drawn instanced in the transparent pass.
export type ParticleEmitter = { playing: boolean, looping: boolean, duration: number, rate: number, rate_over_distance: number, bursts: { { time: number, count: number } }, max_particles: number, lifetime: vector, speed: vector, direction: vector, cone_angle: number, spawn_radius: number, gravity: vector, drag: number, size: { { t: number, value: number } }, size_random: number, color: { { t: number, color: Color } }, intensity: number, spin: vector, random_rotation: boolean, texture: string?, atlas: { x: number, y: number }, blend: "Alpha" | "Additive", soft_distance: number, lit: boolean, space: "World" | "Local", seed: number, despawn_when_finished: boolean }
--- The fields of `ParticleEmitter` to change, each optional (what `set` takes).
export type ParticleEmitterPatch = { playing: boolean?, looping: boolean?, duration: number?, rate: number?, rate_over_distance: number?, bursts: { { time: number, count: number } }?, max_particles: number?, lifetime: vector?, speed: vector?, direction: vector?, cone_angle: number?, spawn_radius: number?, gravity: vector?, drag: number?, size: { { t: number, value: number } }?, size_random: number?, color: { { t: number, color: Color } }?, intensity: number?, spin: vector?, random_rotation: boolean?, texture: string?, atlas: { x: number, y: number }?, blend: ("Alpha" | "Additive")?, soft_distance: number?, lit: boolean?, space: ("World" | "Local")?, seed: number?, despawn_when_finished: boolean? }
--- The player: moves its `KinematicCharacter` with the `move` action (WASD, arrows, left stick) relative to the camera's view, jumps with `jump` (Space, gamepad South), runs while `run` is held (Shift, gamepad West), turns its body toward where it moves and feeds the body's animation state machine (`speed`, `grounded`, `jump`). Below `fall_limit` it respawns at the last checkpoint. It is the player for interactables and the follow camera.
export type PlayerController = { enabled: boolean, speed: number, run_speed: number, jump: boolean, turn_rate: number, fall_limit: number, camera: string?, body: string? }
--- The fields of `PlayerController` to change, each optional (what `set` takes).
export type PlayerControllerPatch = { enabled: boolean?, speed: number?, run_speed: number?, jump: boolean?, turn_rate: number?, fall_limit: number?, camera: string?, body: string? }
--- A point light at the entity's position, emitting equally in all directions.
export type PointLight = { color: Color, intensity: number, range: number, shadows: boolean, shadow: { resolution: number, depth_bias: number, normal_bias: number } }
--- The fields of `PointLight` to change, each optional (what `set` takes).
export type PointLightPatch = { color: Color?, intensity: number?, range: number?, shadows: boolean?, shadow: { resolution: number, depth_bias: number, normal_bias: number }? }
--- Instances a scene asset (a `.scene.json`/`.prefab.json` file, or a glTF's `#Scene`) beneath this entity. Only this component, the entity's own `Name` and `Transform` are saved; the instanced contents are runtime entities, instantiated by the asset glue, never saved, and rebuilt when the source scene changes. No per-instance overrides yet (M4).
export type PrefabInstance = { scene: string? }
--- The fields of `PrefabInstance` to change, each optional (what `set` takes).
export type PrefabInstancePatch = { scene: string? }
--- A built-in shape with parameters. Saved in scenes; the mesh is derived.
export type Primitive = { shape: { Cuboid: { size: vector? } } | { Sphere: { radius: number?, sectors: number?, stacks: number? } } | { Cylinder: { radius: number?, height: number?, segments: number? } } | { Capsule: { radius: number?, half_height: number?, segments: number?, rings: number? } } | { Plane: { size: vector?, cells: { x: number, y: number }? } }, cast_shadows: boolean }
--- The fields of `Primitive` to change, each optional (what `set` takes).
export type PrimitivePatch = { shape: ({ Cuboid: { size: vector? } } | { Sphere: { radius: number?, sectors: number?, stacks: number? } } | { Cylinder: { radius: number?, height: number?, segments: number? } } | { Capsule: { radius: number?, half_height: number?, segments: number?, rings: number? } } | { Plane: { size: vector?, cells: { x: number, y: number }? } })?, cast_shadows: boolean? }
--- Settings of a procedural object, next to its generator component (`Lathe`, `Wall`, ...). Optional: a generator entity without one uses the defaults (seed 0, the entity's own material, shadows on).
export type Procedural = { seed: number, materials: { string? }, cast_shadows: boolean }
--- The fields of `Procedural` to change, each optional (what `set` takes).
export type ProceduralPatch = { seed: number?, materials: { string? }?, cast_shadows: boolean? }
--- Simulates the entity as a rigid body, using the entity's `Collider` (non-trigger) as its shape. Saved in scenes; the simulation state lives in the `Dynamics` resource and is never saved.
export type RigidBody = { kind: "Dynamic" | "Kinematic" | "Fixed", mass: number, linear_damping: number, angular_damping: number, gravity_scale: number, ccd: boolean, can_sleep: boolean, locks: { translate_x: boolean, translate_y: boolean, translate_z: boolean, rotate_x: boolean, rotate_y: boolean, rotate_z: boolean } }
--- The fields of `RigidBody` to change, each optional (what `set` takes).
export type RigidBodyPatch = { kind: ("Dynamic" | "Kinematic" | "Fixed")?, mass: number?, linear_damping: number?, angular_damping: number?, gravity_scale: number?, ccd: boolean?, can_sleep: boolean?, locks: { translate_x: boolean, translate_y: boolean, translate_z: boolean, rotate_x: boolean, rotate_y: boolean, rotate_z: boolean }? }
--- Places instances of meshes or prefabs (species) over an owner-local area polygon and painted regions, on the ground entity or the owner's XZ plane: deterministic per seed and chunk, and regenerated per chunk when its inputs change. The instances are runtime only.
export type Scatter = { species: { { source: { Mesh: { mesh: string?, materials: { string? }? } } | { Prefab: { scene: string? } }, weight: number, scale: vector, random_yaw: boolean, align_to_normal: number, tint_jitter: number, cast_shadows: boolean } }, density: number, min_distance: number, max_slope: number, min_height: number, max_height: number, area: { vector }, surface: string?, seed: number, mask: { cell_size: number, tiles: { { x: number, z: number, cells: string } } } }
--- The fields of `Scatter` to change, each optional (what `set` takes).
export type ScatterPatch = { species: ({ { source: { Mesh: { mesh: string?, materials: { string? }? } } | { Prefab: { scene: string? } }, weight: number, scale: vector, random_yaw: boolean, align_to_normal: number, tint_jitter: number, cast_shadows: boolean } })?, density: number?, min_distance: number?, max_slope: number?, min_height: number?, max_height: number?, area: { vector }?, surface: string?, seed: number?, mask: { cell_size: number, tiles: { { x: number, z: number, cells: string } } }? }
--- Runs a Luau script (a `.luau` file under `assets/`) on this entity while the game plays (the player, Play mode; never while editing). The script's declared properties appear in the inspector; values that differ from the script's defaults are stored here and saved with the scene.
export type Script = { source: string?, properties: { { name: string, value: { Number: number } | { Integer: number } | { Boolean: boolean } | { String: string } | { Vector: vector } | { Color: Color } | { Entity: string? } | { Asset: string? } } } }
--- The fields of `Script` to change, each optional (what `set` takes).
export type ScriptPatch = { source: string?, properties: ({ { name: string, value: { Number: number } | { Integer: number } | { Boolean: boolean } | { String: string } | { Vector: vector } | { Color: Color } | { Entity: string? } | { Asset: string? } } })? }
--- Saved intent of a script rule: the rule file, owner-local points and property values. The rule turns it into derived parts in the editor and in the game.
export type ScriptIntent = { rule: string?, points: { vector }, properties: { { name: string, value: { Number: number } | { Integer: number } | { Boolean: boolean } | { String: string } | { Vector: vector } | { Color: Color } | { Entity: string? } | { Asset: string? } } } }
--- The fields of `ScriptIntent` to change, each optional (what `set` takes).
export type ScriptIntentPatch = { rule: string?, points: { vector }?, properties: ({ { name: string, value: { Number: number } | { Integer: number } | { Boolean: boolean } | { String: string } | { Vector: vector } | { Color: Color } | { Entity: string? } | { Asset: string? } } })? }
--- A material drawn by a custom WGSL shader with editable parameters.
export type ShaderMaterial = { shader: string?, params: { { name: string, value: { Float: number } | { Int: number } | { Uint: number } | { Vec2: vector } | { Vec3: vector } | { Vec4: { x: number, y: number, z: number, w: number } } | { Color: Color } } }, alpha_mode: "Opaque" | { Mask: { cutoff: number? } } | "Blend" | "Additive" | "Multiply" | "Multiply2x", double_sided: boolean, depth_fade: number, sort_bias: number, depth_write: "Auto" | "On" | "Off", depth_test: boolean, prewrite_depth: boolean, scene_depth: boolean, bounds_padding: number }
--- The fields of `ShaderMaterial` to change, each optional (what `set` takes).
export type ShaderMaterialPatch = { shader: string?, params: ({ { name: string, value: { Float: number } | { Int: number } | { Uint: number } | { Vec2: vector } | { Vec3: vector } | { Vec4: { x: number, y: number, z: number, w: number } } | { Color: Color } } })?, alpha_mode: ("Opaque" | { Mask: { cutoff: number? } } | "Blend" | "Additive" | "Multiply" | "Multiply2x")?, double_sided: boolean?, depth_fade: number?, sort_bias: number?, depth_write: ("Auto" | "On" | "Off")?, depth_test: boolean?, prewrite_depth: boolean?, scene_depth: boolean?, bounds_padding: number? }
--- Binds this entity's mesh (its `MeshAsset`) to a skeleton: the skin asset and the joint entities, by uid, in the skin's joint order. Saved; the glTF importer writes it into model scenes, and prefab instancing remaps the uids to the instance's joints. The asset glue turns it into a runtime `SkinnedMesh`; joints that do not exist are left at the mesh's bind pose (with a warning).
export type SkinBinding = { skin: string?, joints: { string } }
--- The fields of `SkinBinding` to change, each optional (what `set` takes).
export type SkinBindingPatch = { skin: string?, joints: { string }? }
--- Makes copies of its template every `interval` seconds while fewer than `max_alive` are around, at random spots within `radius` of itself. The template is the entity in `template`, else the spawner's first child; it is hidden while playing, so keep it where the spawner is. Copies are not saved: a save game or Restart starts the spawner again.
export type Spawner = { enabled: boolean, template: string?, interval: number, max_alive: number, total: number, radius: number, lifetime: number }
--- The fields of `Spawner` to change, each optional (what `set` takes).
export type SpawnerPatch = { enabled: boolean?, template: string?, interval: number?, max_alive: number?, total: number?, radius: number?, lifetime: number? }
--- Turns the entity around an axis, forever (coins, fans, windmills).
export type Spinner = { enabled: boolean, axis: vector, speed: number }
--- The fields of `Spinner` to change, each optional (what `set` takes).
export type SpinnerPatch = { enabled: boolean?, axis: vector?, speed: number? }
--- A 3D spline: control points, interpolation mode, open or closed. Plain parameter data for geometry functions, and a saved component whose points are in its entity's local space (see the module docs).
export type Spline = { points: { { position: vector, in_tangent: vector?, out_tangent: vector?, roll: number, scale: vector } }, mode: "CatmullRom" | "Bezier" | "Polyline", closed: boolean }
--- The fields of `Spline` to change, each optional (what `set` takes).
export type SplinePatch = { points: { { position: vector, in_tangent: vector?, out_tangent: vector?, roll: number, scale: vector } }?, mode: ("CatmullRom" | "Bezier" | "Polyline")?, closed: boolean? }
--- A spot light at the entity's position, shining down the entity's −Z (`GlobalTransform::forward()`) in a cone.
export type SpotLight = { color: Color, intensity: number, range: number, inner_angle: number, outer_angle: number, shadows: boolean, shadow: { resolution: number, depth_bias: number, normal_bias: number } }
--- The fields of `SpotLight` to change, each optional (what `set` takes).
export type SpotLightPatch = { color: Color?, intensity: number?, range: number?, inner_angle: number?, outer_angle: number?, shadows: boolean?, shadow: { resolution: number, depth_bias: number, normal_bias: number }? }
--- The built-in lit material: metallic/roughness PBR parameters and optional textures. Vertex colours multiply `base_color`.
export type StandardMaterial = { base_color: Color, roughness: number, metallic: number, emissive: Color, base_color_texture: string?, normal_texture: string?, orm_texture: string?, emissive_texture: string?, alpha_mode: "Opaque" | { Mask: { cutoff: number? } } | "Blend" | "Additive" | "Multiply" | "Multiply2x", double_sided: boolean, depth_fade: number, wind: { strength: number, frequency: number, weight: { HeightRamp: { bottom: number?, top: number? } } | "VertexAlpha" }, uv_scale: vector, sort_bias: number, depth_write: "Auto" | "On" | "Off", depth_test: boolean, prewrite_depth: boolean }
--- The fields of `StandardMaterial` to change, each optional (what `set` takes).
export type StandardMaterialPatch = { base_color: Color?, roughness: number?, metallic: number?, emissive: Color?, base_color_texture: string?, normal_texture: string?, orm_texture: string?, emissive_texture: string?, alpha_mode: ("Opaque" | { Mask: { cutoff: number? } } | "Blend" | "Additive" | "Multiply" | "Multiply2x")?, double_sided: boolean?, depth_fade: number?, wind: ({ strength: number, frequency: number, weight: { HeightRamp: { bottom: number?, top: number? } } | "VertexAlpha" })?, uv_scale: vector?, sort_bias: number?, depth_write: ("Auto" | "On" | "Off")?, depth_test: boolean?, prewrite_depth: boolean? }
--- A profile swept along a spline: tubes, rails, kerbs, walls, paths. Reads the `Spline` on this entity, or on the entity named by `spline`; editing that spline regenerates the sweep.
export type Sweep = { spline: string?, profile: { Rect: { width: number?, height: number? } } | { Circle: { radius: number?, segments: number? } } | { Custom: { points: { vector }?, closed: boolean? } }, tolerance: number, orientation: "Up" | "RotationMinimizing", caps: boolean, follow_ground: boolean, ground_offset: number }
--- The fields of `Sweep` to change, each optional (what `set` takes).
export type SweepPatch = { spline: string?, profile: ({ Rect: { width: number?, height: number? } } | { Circle: { radius: number?, segments: number? } } | { Custom: { points: { vector }?, closed: boolean? } })?, tolerance: number?, orientation: ("Up" | "RotationMinimizing")?, caps: boolean?, follow_ground: boolean?, ground_offset: number? }
--- Heightfield ground shaped by `TerrainStamp`s and drawn in chunks (see the module docs). The terrain covers `size` metres on X and Z, centred on its entity.
export type Terrain = { size: vector, cell_size: number, base_height: number, stamps: { { Hill: { center: vector?, radius: number?, height: number? } } | { Ridge: { points: { vector }?, width: number?, height: number? } } | { Basin: { center: vector?, radius: number?, depth: number? } } | { Flatten: { center: vector?, radius: number?, height: number?, bank: number? } } | { FlattenPath: { points: { vector }?, width: number?, bank: number? } } | { Noise: { seed: number?, scale: number?, amplitude: number?, octaves: number? } } }, layers: { grass: Color, dirt: Color, rock: Color, rock_slope: number, variation: number }, skirt: number, collide: boolean, show_walkability: boolean, walk_slope: number }
--- The fields of `Terrain` to change, each optional (what `set` takes).
export type TerrainPatch = { size: vector?, cell_size: number?, base_height: number?, stamps: ({ { Hill: { center: vector?, radius: number?, height: number? } } | { Ridge: { points: { vector }?, width: number?, height: number? } } | { Basin: { center: vector?, radius: number?, depth: number? } } | { Flatten: { center: vector?, radius: number?, height: number?, bank: number? } } | { FlattenPath: { points: { vector }?, width: number?, bank: number? } } | { Noise: { seed: number?, scale: number?, amplitude: number?, octaves: number? } } })?, layers: { grass: Color, dirt: Color, rock: Color, rock_slope: number, variation: number }?, skirt: number?, collide: boolean?, show_walkability: boolean?, walk_slope: number? }
--- One chunk of a `Terrain`: generated by the `terrain` rule as a part of the terrain's entity, never saved. It holds the stamps that reach it, so editing a stamp regenerates only the chunks under it.
export type TerrainChunk = { origin: vector, cells_x: number, cells_z: number, cell_size: number, base_height: number, stamps: { { Hill: { center: vector?, radius: number?, height: number? } } | { Ridge: { points: { vector }?, width: number?, height: number? } } | { Basin: { center: vector?, radius: number?, depth: number? } } | { Flatten: { center: vector?, radius: number?, height: number?, bank: number? } } | { FlattenPath: { points: { vector }?, width: number?, bank: number? } } | { Noise: { seed: number?, scale: number?, amplitude: number?, octaves: number? } } }, layers: { grass: Color, dirt: Color, rock: Color, rock_slope: number, variation: number }, skirt: number, skirt_edges: number, show_walkability: boolean, walk_slope: number }
--- The fields of `TerrainChunk` to change, each optional (what `set` takes).
export type TerrainChunkPatch = { origin: vector?, cells_x: number?, cells_z: number?, cell_size: number?, base_height: number?, stamps: ({ { Hill: { center: vector?, radius: number?, height: number? } } | { Ridge: { points: { vector }?, width: number?, height: number? } } | { Basin: { center: vector?, radius: number?, depth: number? } } | { Flatten: { center: vector?, radius: number?, height: number?, bank: number? } } | { FlattenPath: { points: { vector }?, width: number?, bank: number? } } | { Noise: { seed: number?, scale: number?, amplitude: number?, octaves: number? } } })?, layers: { grass: Color, dirt: Color, rock: Color, rock_slope: number, variation: number }?, skirt: number?, skirt_edges: number?, show_walkability: boolean?, walk_slope: number? }
--- A dirt path following the XZ course of the `Spline` on this entity. Paths on the same terrain merge wherever their footprints touch.
export type TerrainPath = { width: number, terrain: string? }
--- The fields of `TerrainPath` to change, each optional (what `set` takes).
export type TerrainPathPatch = { width: number?, terrain: string? }
--- Runtime mesh parameters of one path chunk, owned by the terrain.
export type TerrainPathChunk = { chunk: { origin: vector, cells_x: number, cells_z: number, cell_size: number, base_height: number, stamps: { { Hill: { center: vector?, radius: number?, height: number? } } | { Ridge: { points: { vector }?, width: number?, height: number? } } | { Basin: { center: vector?, radius: number?, depth: number? } } | { Flatten: { center: vector?, radius: number?, height: number?, bank: number? } } | { FlattenPath: { points: { vector }?, width: number?, bank: number? } } | { Noise: { seed: number?, scale: number?, amplitude: number?, octaves: number? } } }, layers: { grass: Color, dirt: Color, rock: Color, rock_slope: number, variation: number }, skirt: number, skirt_edges: number, show_walkability: boolean, walk_slope: number }, triangles: { { vector } } }
--- The fields of `TerrainPathChunk` to change, each optional (what `set` takes).
export type TerrainPathChunkPatch = { chunk: ({ origin: vector, cells_x: number, cells_z: number, cell_size: number, base_height: number, stamps: { { Hill: { center: vector?, radius: number?, height: number? } } | { Ridge: { points: { vector }?, width: number?, height: number? } } | { Basin: { center: vector?, radius: number?, depth: number? } } | { Flatten: { center: vector?, radius: number?, height: number?, bank: number? } } | { FlattenPath: { points: { vector }?, width: number?, bank: number? } } | { Noise: { seed: number?, scale: number?, amplitude: number?, octaves: number? } } }, layers: { grass: Color, dirt: Color, rock: Color, rock_slope: number, variation: number }, skirt: number, skirt_edges: number, show_walkability: boolean, walk_slope: number })?, triangles: { { vector } }? }
--- Local transform relative to the parent (or the world, for roots).
export type Transform = { translation: vector, rotation: Quat, scale: vector }
--- The fields of `Transform` to change, each optional (what `set` takes).
export type TransformPatch = { translation: vector?, rotation: Quat?, scale: vector? }
--- Component: a runtime-only entity (pooled projectiles, spawned enemies, effects). It and its descendants are never written to scene or save files, and loading a save game despawns them (the loaded state does not contain them). Scripts spawn one with `world.spawn{ ..., transient = true }`. Not shown in the add-component menu.
export type Transient = {}
--- The fields of `Transient` to change, each optional (what `set` takes).
export type TransientPatch = {}
--- Persistent entity identity, stable across save, load, Play and undo.
export type Uid = string
--- The fields of `Uid` to change, each optional (what `set` takes).
export type UidPatch = string
--- A body's velocity. Saved values are the initial velocity; while the simulation runs it writes the current velocity here every fixed step, and a value you write is applied to the body at the next step. On a kinematic body it moves the body (a moving platform), in world space.
export type Velocity = { linear: vector, angular: vector }
--- The fields of `Velocity` to change, each optional (what `set` takes).
export type VelocityPatch = { linear: vector?, angular: vector? }
--- Whether this entity and its descendants are drawn (default `true`). A hidden entity's meshes are not drawn, its particle emitters stop (their particles are dropped) and picking ignores it; physics and scripts are unaffected. A hidden parent hides its children whatever theirs say.
export type Visible = boolean
--- The fields of `Visible` to change, each optional (what `set` takes).
export type VisiblePatch = boolean

-- Resources
--- Project settings resource (`assets/settings/AudioSettings.json`, Game ▸ Project Settings in the editor): the master and bus volumes. Hot-reloads, also during Play.
export type AudioSettings = { master: number, effects: number, ambience: number, music: number }
--- The fields of `AudioSettings` to change, each optional (what `set` takes).
export type AudioSettingsPatch = { master: number?, effects: number?, ambience: number?, music: number? }
--- Sky, ambient light, fog, exposure and post-processing (ambient occlusion, bloom, colour grading), atmosphere (haze, mist, quieting), vignette and wind. A scene resource; the default is `Environment::day`. Fields missing from a scene file take their default values. Every value is a uniform: animating them (time of day) never rebuilds a pipeline.
export type Environment = { sky_zenith: Color, sky_horizon: Color, ground: Color, ambient_intensity: number, sun_disc_size: number, fog_density: number, fog_height_falloff: number, fog_base_height: number, exposure: number, tonemapper: "AgX" | "AcesFitted" | "None" | "Neutral", ambient_occlusion: { enabled: boolean, intensity: number, radius: number, bias: number, power: number, direct_lighting: number }, bloom: { enabled: boolean, intensity: number, scatter: number, threshold: number, knee: number }, color_grading: { temperature: number, tint: number, contrast: number, saturation: number, shadows: vector, midtones: vector, highlights: vector, shadows_end: number, highlights_start: number, lift: vector, gamma: vector, gain: vector }, sky: { gradient: "Simple" | { Layered: { upper: Color?, below_horizon: Color? } }, sun_glow: number, sun_disc: number, horizon_glow: number, stars: number }, aerial_haze: { enabled: boolean, color: Color, density: number, start_distance: number, max_opacity: number, sun_scatter: number, sky_height: number }, height_mist: { enabled: boolean, color: Color, top: number, bottom: number, max_opacity: number, near_fade: number }, distance_quieting: { enabled: boolean, start: number, ["end"]: number, desaturate: number, contrast: number, tone: Color }, vignette: { intensity: number, smoothness: number, rounded: boolean }, wind: { direction: vector, strength: number, gust_length: number, gust_speed: number } }
--- The fields of `Environment` to change, each optional (what `set` takes).
export type EnvironmentPatch = { sky_zenith: Color?, sky_horizon: Color?, ground: Color?, ambient_intensity: number?, sun_disc_size: number?, fog_density: number?, fog_height_falloff: number?, fog_base_height: number?, exposure: number?, tonemapper: ("AgX" | "AcesFitted" | "None" | "Neutral")?, ambient_occlusion: { enabled: boolean, intensity: number, radius: number, bias: number, power: number, direct_lighting: number }?, bloom: { enabled: boolean, intensity: number, scatter: number, threshold: number, knee: number }?, color_grading: { temperature: number, tint: number, contrast: number, saturation: number, shadows: vector, midtones: vector, highlights: vector, shadows_end: number, highlights_start: number, lift: vector, gamma: vector, gain: vector }?, sky: ({ gradient: "Simple" | { Layered: { upper: Color?, below_horizon: Color? } }, sun_glow: number, sun_disc: number, horizon_glow: number, stars: number })?, aerial_haze: { enabled: boolean, color: Color, density: number, start_distance: number, max_opacity: number, sun_scatter: number, sky_height: number }?, height_mist: { enabled: boolean, color: Color, top: number, bottom: number, max_opacity: number, near_fade: number }?, distance_quieting: { enabled: boolean, start: number, ["end"]: number, desaturate: number, contrast: number, tone: Color }?, vignette: { intensity: number, smoothness: number, rounded: boolean }?, wind: { direction: vector, strength: number, gust_length: number, gust_speed: number }? }
--- What winning and losing mean in this scene. A scene resource (edit it in Scene Settings, or with `set_resource`); a scene without it has no built-in win, lose, pause or end screens.
export type GameRules = { win: { { AtLeast: { var: string?, value: number? } } | { Below: { var: string?, value: number? } } | { Equals: { var: string?, value: number? } } | { GroupDone: { group: string? } } | { TimeAbove: { seconds: number? } } }, lose: { { AtLeast: { var: string?, value: number? } } | { Below: { var: string?, value: number? } } | { Equals: { var: string?, value: number? } } | { GroupDone: { group: string? } } | { TimeAbove: { seconds: number? } } }, time_limit: number, lives: number, menus: boolean, next_scene: string, win_title: string, lose_title: string }
--- The fields of `GameRules` to change, each optional (what `set` takes).
export type GameRulesPatch = { win: ({ { AtLeast: { var: string?, value: number? } } | { Below: { var: string?, value: number? } } | { Equals: { var: string?, value: number? } } | { GroupDone: { group: string? } } | { TimeAbove: { seconds: number? } } })?, lose: ({ { AtLeast: { var: string?, value: number? } } | { Below: { var: string?, value: number? } } | { Equals: { var: string?, value: number? } } | { GroupDone: { group: string? } } | { TimeAbove: { seconds: number? } } })?, time_limit: number?, lives: number?, menus: boolean?, next_scene: string?, win_title: string?, lose_title: string? }
--- The state of the current game: phase, time, lives, checkpoint. A runtime resource of playing worlds (never in scene files; save games keep it through the `"game"` hook). Live tools read it with `world_query`.
export type GameState = { phase: "Playing" | "Paused" | "Won" | "Lost", elapsed: number, lives_left: number, checkpoint: vector?, respawns: number }
--- The fields of `GameState` to change, each optional (what `set` takes).
export type GameStatePatch = { phase: ("Playing" | "Paused" | "Won" | "Lost")?, elapsed: number?, lives_left: number?, checkpoint: vector?, respawns: number? }
--- Named numbers for puzzle and progress state (`embers`, `gate_open`); unset names read 0. A resource of playing worlds: it is kept across scene switches, saved in save games (hook `"vars"`), not stored in scene files. Scripts use the `vars` global; live tools read it with `world_query` (`include_resources`).
export type GameVars = { values: { [string]: number } }
--- The fields of `GameVars` to change, each optional (what `set` takes).
export type GameVarsPatch = { values: { [string]: number }? }
--- The built-in HUD: a panel in the top-left corner with game vars, group progress, lives and the time left. A scene resource; without it there is no built-in HUD. Styled by the theme's `hud` style.
export type HudSettings = { enabled: boolean, vars: { { var: string, label: string } }, groups: boolean, lives: boolean, timer: boolean, health: boolean }
--- The fields of `HudSettings` to change, each optional (what `set` takes).
export type HudSettingsPatch = { enabled: boolean?, vars: { { var: string, label: string } }?, groups: boolean?, lives: boolean?, timer: boolean?, health: boolean? }
--- How characters that are not the player find their way: the size of the agent the walkable area is computed for and how finely it is sampled. A scene resource (Scene Settings); without it the defaults apply, which match the default `KinematicCharacter`.
export type NavSettings = { enabled: boolean, agent_radius: number, agent_height: number, max_slope: number, step_height: number, cell_size: number, tile_size: number }
--- The fields of `NavSettings` to change, each optional (what `set` takes).
export type NavSettingsPatch = { enabled: boolean?, agent_radius: number?, agent_height: number?, max_slope: number?, step_height: number?, cell_size: number?, tile_size: number? }
--- Project-wide simulation settings: a settings resource (`assets/settings/PhysicsSettings.json`, Game ▸ Project Settings).
export type PhysicsSettings = { gravity: vector, solver_iterations: number }
--- The fields of `PhysicsSettings` to change, each optional (what `set` takes).
export type PhysicsSettingsPatch = { gravity: vector?, solver_iterations: number? }

-- Events
--- Sent each time an `Interactable` is used, after its actions ran. Scripts: `events.on("Interacted", function(e) ... end)`, with `e.entity` and `e.interactor` uids (`world.entity(uid)`).
export type Interacted = { entity: string?, interactor: string? }
--- The fields of `Interacted` to change, each optional (what `set` takes).
export type InteractedPatch = { entity: string?, interactor: string? }
--- Event: play a clip once. Send it with `world.send(PlaySound::new(..))` from any stage; it starts in the same frame's `PostUpdate` (a clip that is not loaded yet starts when it is, or is dropped with a warning after `PENDING_TIMEOUT` seconds).
export type PlaySound = { clip: string?, position: vector?, volume: number, pitch: number, bus: "Effects" | "Ambience" | "Music", min_distance: number, max_distance: number }
--- The fields of `PlaySound` to change, each optional (what `set` takes).
export type PlaySoundPatch = { clip: string?, position: vector?, volume: number?, pitch: number?, bus: ("Effects" | "Ambience" | "Music")?, min_distance: number?, max_distance: number? }

-- core
--- A rotation (unit quaternion). Values are immutable: every operation returns a new Quat. `q * v` rotates a vector, `q * r` applies `r` first, then `q`. Compares with `==`.
declare extern type Quat with
    --- The x component.
    x: number
    --- The y component.
    y: number
    --- The z component.
    z: number
    --- The w (scalar) component.
    w: number
    --- The opposite rotation.
    function inverse(self): Quat
    --- The same rotation scaled back to unit length (after many multiplications).
    function normalize(self): Quat
    --- Spherical interpolation from this rotation (t = 0) to `to` (t = 1).
    function slerp(self, to: Quat, t: number): Quat
    --- The angle between two rotations, in radians.
    function angle_to(self, other: Quat): number
    --- The rotated forward direction (-Z).
    function forward(self): vector
    --- The rotated right direction (+X).
    function right(self): vector
    --- The rotated up direction (+Y).
    function up(self): vector
    --- Euler angles in radians, as `x, y, z` (pitch, yaw, roll in YXZ order), the inverse of `Quat.from_euler`.
    function to_euler(self): (number, number, number)
    --- The rotation axis (unit vector) and angle in radians.
    function to_axis_angle(self): (vector, number)
    --- `q * r`: the rotation `r` followed by `q`.
    function __mul(self, other: Quat): Quat
    --- `q * v`: the vector `v` rotated by `q`.
    function __mul(self, other: vector): vector
end

--- Constructors of rotations.
declare Quat: {
    --- No rotation.
    identity: () -> Quat,
    --- A rotation from raw components (normalized).
    new: (x: number, y: number, z: number, w: number) -> Quat,
    --- A rotation of `radians` around `axis` (normalized; +Y when zero).
    from_axis_angle: (axis: vector, radians: number) -> Quat,
    --- A rotation from Euler angles in radians: pitch `x`, yaw `y` and roll `z`, in YXZ order (yaw, then pitch, then roll, each about the already turned axes).
    from_euler: (x: number, y: number, z: number) -> Quat,
    --- A rotation of `radians` around the X axis.
    from_rotation_x: (radians: number) -> Quat,
    --- A rotation of `radians` around the Y axis (yaw; positive turns left, from -Z towards -X).
    from_rotation_y: (radians: number) -> Quat,
    --- A rotation of `radians` around the Z axis.
    from_rotation_z: (radians: number) -> Quat,
    --- The rotation whose forward (-Z) points along `direction`, with `up` (default +Y) as close to up as possible.
    look_to: (direction: vector, up: vector?) -> Quat,
}

--- A colour with linear channels from 0 to 1 (alpha 1 is opaque). Components store colours this way; build one with `Color.rgb`, `Color.srgb` or `Color.hex`, or write a table `{ r = 1, g = 0.5, b = 0, a = 1 }`.
export type Color = {
    --- Red, linear.
    r: number,
    --- Green, linear.
    g: number,
    --- Blue, linear.
    b: number,
    --- Alpha (opacity).
    a: number,
}

--- Constructors of colours.
declare Color: {
    --- A colour from linear channels (alpha defaults to 1).
    rgb: (r: number, g: number, b: number, a: number?) -> Color,
    --- A colour from sRGB channels from 0 to 1, as colour pickers show them, converted to linear (alpha defaults to 1).
    srgb: (r: number, g: number, b: number, a: number?) -> Color,
    --- A colour from an sRGB hex string, "#RRGGBB" or "RRGGBB".
    hex: (hex: string) -> Color,
}

--- Messages in the editor's Console and the terminal, prefixed with the script's path. Values are converted with `tostring` and joined with spaces. `print(...)` is `log.info(...)`.
declare log: {
    --- An information message.
    info: (...any) -> (),
    --- A warning: also an editor toast and a Console badge count.
    warn: (...any) -> (),
    --- An error: also an editor toast and a Console badge count. It does not stop the script (use Luau's `error()` for that).
    error: (...any) -> (),
}

--- Suspends the running task for `seconds` of game time (default 0) and returns the seconds actually waited. Works in `on_start` and in `task.spawn` / `task.delay` functions; elsewhere it fails. Pausing or scaling time pauses or scales waits.
declare function wait(seconds: number?): number
--- Suspends the running task for `frames` frames (default 1). Same rules as `wait`.
declare function wait_frames(frames: number?): ()
--- Suspends the running task until `condition()` returns true (checked once per frame) and returns true, or returns false after `timeout` seconds of game time. Same rules as `wait`.
declare function wait_until(condition: () -> boolean, timeout: number?): boolean

--- Tasks: functions that run alongside the frame loop and may `wait`. A task belongs to the script instance that started it and stops with it (or when its file reloads with `on_reload`).
declare task: {
    --- Runs `f(...)` at once until its first wait, then resumes it when the wait ends (in Update, on game time). Returns the task id for `task.cancel`.
    spawn: <A...>(f: (A...) -> ...any, A...) -> number,
    --- Runs `f(...)` as a task after `seconds` of game time. Returns the task id.
    delay: <A...>(seconds: number, f: (A...) -> ...any, A...) -> number,
    --- Stops a waiting task. Returns true if it was still waiting.
    cancel: (id: number) -> boolean,
}

-- Entity
--- A handle to an entity (an object in the scene). Handles compare by value (`==`) and work as table keys; a handle to a despawned entity stays a valid value but its methods fail ("the entity no longer exists"), so check `is_alive` when you keep handles. Position methods work on the local `Transform` (relative to the parent) unless named `world_`.
declare extern type Entity with
    --- True while the entity exists.
    function is_alive(self): boolean
    --- A runtime id for logs and debugging: unique while the game runs, not saved (use `uid` to refer to an entity across saves).
    function id(self): string
    --- The entity's persistent uid (saved in scenes and save games), or nil if it has none.
    function uid(self): string?
    --- The entity's `Name`, or nil.
    function name(self): string?
    --- Sets the entity's `Name`. Returns false if the entity no longer exists.
    function set_name(self, name: string): boolean
    --- True if the entity has the registered component `component` (fails with a did-you-mean for unknown names).
    function has(self, component: string): boolean
    --- Adds component `component` with its default values patched with `value` (a table of fields), or patches it when the entity already has it. `Script` takes a path or `{ source = path, props = { ... } }`.
    function add(self, component: string, value: any?): ()
    --- Removes component `component`. Returns true if the entity had it.
    function remove(self, component: string): boolean
    --- Removes the entity and its descendants. Returns false if it was already gone.
    function despawn(self): boolean
    --- Shows or hides this entity and its descendants (`Visible`, saved): hidden meshes and lights are not drawn and hidden particle emitters stop; physics and scripts keep running.
    function set_visible(self, visible: boolean): ()
    --- False when this entity or an ancestor is hidden (`Visible(false)`).
    function is_visible(self): boolean
    --- The parent entity, or nil for a root.
    function parent(self): Entity?
    --- The child entities, in hierarchy order.
    function children(self): { Entity }
    --- Re-parents the entity (nil makes it a root). Its local transform is kept, so it moves with its new parent.
    function set_parent(self, parent: Entity?): ()
    --- The `self` table of the script running on this entity (its fields and module functions), or nil when it has none.
    function script(self): any
    --- The local position (`Transform.translation`).
    function position(self): vector
    --- Sets the local position.
    function set_position(self, position: vector): ()
    --- Moves the entity by `offset` (local axes of the parent, metres).
    function translate(self, offset: vector): ()
    --- The local rotation.
    function rotation(self): Quat
    --- Sets the local rotation.
    function set_rotation(self, rotation: Quat): ()
    --- Applies `rotation` on top of the current one (`rotation * current`, about the parent's axes).
    function rotate(self, rotation: Quat): ()
    --- Turns the entity around the parent's Y axis by `radians` (positive turns left).
    function rotate_y(self, radians: number): ()
    --- The local scale.
    function scale(self): vector
    --- Sets the local scale: a vector, or a number for a uniform scale.
    function set_scale(self, scale: vector | number): ()
    --- The local forward direction (-Z rotated by the local rotation).
    function forward(self): vector
    --- The local right direction (+X rotated).
    function right(self): vector
    --- The local up direction (+Y rotated).
    function up(self): vector
    --- Turns the entity so its forward (-Z) points at `target`, a point in the world (also under a moved or turned parent), keeping `up` (default +Y, in the world) up.
    function look_at(self, target: vector, up: vector?): ()
    --- The position in the world (through every parent), computed now.
    function world_position(self): vector
    --- The rotation in the world (through every parent), computed now.
    function world_rotation(self): Quat
    --- Where an `Interpolated` entity is drawn this frame: between its last two fixed-step positions. Use it to follow an entity moved in `on_fixed_update` smoothly (a camera).
    function interpolated_position(self): vector
    -- nav
    --- Walks this character to `position` along the navmesh, around walls, through doors and up stairs (adds a `NavAgent` if it has none; needs a `KinematicCharacter`). Call it again to change the goal: a goal that moved less than a metre does not replan.
    function move_to(self, position: vector): ()
    --- Stops walking (clears the `NavAgent` destination).
    function stop(self): ()
    --- Where its walk stands: "idle", "baking" (the navmesh is not ready: it walks straight), "moving", "arrived", "unreachable" (it walks as close as it can) or "off_mesh".
    function nav_status(self): string
    --- A copy of this entity's `AmbientVolume` component, or nil when it has none.
    function get(self, component: "AmbientVolume"): AmbientVolume?
    --- Patches this entity's `AmbientVolume` component with the given fields.
    function set(self, component: "AmbientVolume", value: AmbientVolumePatch): ()
    --- A copy of this entity's `AnimationPlayer` component, or nil when it has none.
    function get(self, component: "AnimationPlayer"): AnimationPlayer?
    --- Patches this entity's `AnimationPlayer` component with the given fields.
    function set(self, component: "AnimationPlayer", value: AnimationPlayerPatch): ()
    --- A copy of this entity's `AnimationStateMachine` component, or nil when it has none.
    function get(self, component: "AnimationStateMachine"): AnimationStateMachine?
    --- Patches this entity's `AnimationStateMachine` component with the given fields.
    function set(self, component: "AnimationStateMachine", value: AnimationStateMachinePatch): ()
    --- A copy of this entity's `AudioListener` component, or nil when it has none.
    function get(self, component: "AudioListener"): AudioListener?
    --- Patches this entity's `AudioListener` component with the given fields.
    function set(self, component: "AudioListener", value: AudioListenerPatch): ()
    --- A copy of this entity's `AudioSource` component, or nil when it has none.
    function get(self, component: "AudioSource"): AudioSource?
    --- Patches this entity's `AudioSource` component with the given fields.
    function set(self, component: "AudioSource", value: AudioSourcePatch): ()
    --- A copy of this entity's `Bobber` component, or nil when it has none.
    function get(self, component: "Bobber"): Bobber?
    --- Patches this entity's `Bobber` component with the given fields.
    function set(self, component: "Bobber", value: BobberPatch): ()
    --- A copy of this entity's `Camera` component, or nil when it has none.
    function get(self, component: "Camera"): Camera?
    --- Patches this entity's `Camera` component with the given fields.
    function set(self, component: "Camera", value: CameraPatch): ()
    --- A copy of this entity's `Chaser` component, or nil when it has none.
    function get(self, component: "Chaser"): Chaser?
    --- Patches this entity's `Chaser` component with the given fields.
    function set(self, component: "Chaser", value: ChaserPatch): ()
    --- A copy of this entity's `Collidable` component, or nil when it has none.
    function get(self, component: "Collidable"): Collidable?
    --- Patches this entity's `Collidable` component with the given fields.
    function set(self, component: "Collidable", value: CollidablePatch): ()
    --- A copy of this entity's `Collider` component, or nil when it has none.
    function get(self, component: "Collider"): Collider?
    --- Patches this entity's `Collider` component with the given fields.
    function set(self, component: "Collider", value: ColliderPatch): ()
    --- A copy of this entity's `DirectionalLight` component, or nil when it has none.
    function get(self, component: "DirectionalLight"): DirectionalLight?
    --- Patches this entity's `DirectionalLight` component with the given fields.
    function set(self, component: "DirectionalLight", value: DirectionalLightPatch): ()
    --- A copy of this entity's `Extrude` component, or nil when it has none.
    function get(self, component: "Extrude"): Extrude?
    --- Patches this entity's `Extrude` component with the given fields.
    function set(self, component: "Extrude", value: ExtrudePatch): ()
    --- A copy of this entity's `FollowCamera` component, or nil when it has none.
    function get(self, component: "FollowCamera"): FollowCamera?
    --- Patches this entity's `FollowCamera` component with the given fields.
    function set(self, component: "FollowCamera", value: FollowCameraPatch): ()
    --- A copy of this entity's `FollowGround` component, or nil when it has none.
    function get(self, component: "FollowGround"): FollowGround?
    --- Patches this entity's `FollowGround` component with the given fields.
    function set(self, component: "FollowGround", value: FollowGroundPatch): ()
    --- A copy of this entity's `Health` component, or nil when it has none.
    function get(self, component: "Health"): Health?
    --- Patches this entity's `Health` component with the given fields.
    function set(self, component: "Health", value: HealthPatch): ()
    --- A copy of this entity's `Interactable` component, or nil when it has none.
    function get(self, component: "Interactable"): Interactable?
    --- Patches this entity's `Interactable` component with the given fields.
    function set(self, component: "Interactable", value: InteractablePatch): ()
    --- A copy of this entity's `Interactor` component, or nil when it has none.
    function get(self, component: "Interactor"): Interactor?
    --- Patches this entity's `Interactor` component with the given fields.
    function set(self, component: "Interactor", value: InteractorPatch): ()
    --- A copy of this entity's `Interpolated` component, or nil when it has none.
    function get(self, component: "Interpolated"): Interpolated?
    --- Patches this entity's `Interpolated` component with the given fields.
    function set(self, component: "Interpolated", value: InterpolatedPatch): ()
    --- A copy of this entity's `Joint` component, or nil when it has none.
    function get(self, component: "Joint"): Joint?
    --- Patches this entity's `Joint` component with the given fields.
    function set(self, component: "Joint", value: JointPatch): ()
    --- A copy of this entity's `KinematicCharacter` component, or nil when it has none.
    function get(self, component: "KinematicCharacter"): KinematicCharacter?
    --- Patches this entity's `KinematicCharacter` component with the given fields.
    function set(self, component: "KinematicCharacter", value: KinematicCharacterPatch): ()
    --- A copy of this entity's `Lathe` component, or nil when it has none.
    function get(self, component: "Lathe"): Lathe?
    --- Patches this entity's `Lathe` component with the given fields.
    function set(self, component: "Lathe", value: LathePatch): ()
    --- A copy of this entity's `MaterialRef` component, or nil when it has none.
    function get(self, component: "MaterialRef"): MaterialRef?
    --- Patches this entity's `MaterialRef` component with the given fields.
    function set(self, component: "MaterialRef", value: MaterialRefPatch): ()
    --- A copy of this entity's `MeshAsset` component, or nil when it has none.
    function get(self, component: "MeshAsset"): MeshAsset?
    --- Patches this entity's `MeshAsset` component with the given fields.
    function set(self, component: "MeshAsset", value: MeshAssetPatch): ()
    --- A copy of this entity's `Mover` component, or nil when it has none.
    function get(self, component: "Mover"): Mover?
    --- Patches this entity's `Mover` component with the given fields.
    function set(self, component: "Mover", value: MoverPatch): ()
    --- A copy of this entity's `Name` component, or nil when it has none.
    function get(self, component: "Name"): Name?
    --- Patches this entity's `Name` component with the given fields.
    function set(self, component: "Name", value: NamePatch): ()
    --- A copy of this entity's `NavAgent` component, or nil when it has none.
    function get(self, component: "NavAgent"): NavAgent?
    --- Patches this entity's `NavAgent` component with the given fields.
    function set(self, component: "NavAgent", value: NavAgentPatch): ()
    --- A copy of this entity's `Overrides` component, or nil when it has none.
    function get(self, component: "Overrides"): Overrides?
    --- Patches this entity's `Overrides` component with the given fields.
    function set(self, component: "Overrides", value: OverridesPatch): ()
    --- A copy of this entity's `ParticleEmitter` component, or nil when it has none.
    function get(self, component: "ParticleEmitter"): ParticleEmitter?
    --- Patches this entity's `ParticleEmitter` component with the given fields.
    function set(self, component: "ParticleEmitter", value: ParticleEmitterPatch): ()
    --- A copy of this entity's `PlayerController` component, or nil when it has none.
    function get(self, component: "PlayerController"): PlayerController?
    --- Patches this entity's `PlayerController` component with the given fields.
    function set(self, component: "PlayerController", value: PlayerControllerPatch): ()
    --- A copy of this entity's `PointLight` component, or nil when it has none.
    function get(self, component: "PointLight"): PointLight?
    --- Patches this entity's `PointLight` component with the given fields.
    function set(self, component: "PointLight", value: PointLightPatch): ()
    --- A copy of this entity's `PrefabInstance` component, or nil when it has none.
    function get(self, component: "PrefabInstance"): PrefabInstance?
    --- Patches this entity's `PrefabInstance` component with the given fields.
    function set(self, component: "PrefabInstance", value: PrefabInstancePatch): ()
    --- A copy of this entity's `Primitive` component, or nil when it has none.
    function get(self, component: "Primitive"): Primitive?
    --- Patches this entity's `Primitive` component with the given fields.
    function set(self, component: "Primitive", value: PrimitivePatch): ()
    --- A copy of this entity's `Procedural` component, or nil when it has none.
    function get(self, component: "Procedural"): Procedural?
    --- Patches this entity's `Procedural` component with the given fields.
    function set(self, component: "Procedural", value: ProceduralPatch): ()
    --- A copy of this entity's `RigidBody` component, or nil when it has none.
    function get(self, component: "RigidBody"): RigidBody?
    --- Patches this entity's `RigidBody` component with the given fields.
    function set(self, component: "RigidBody", value: RigidBodyPatch): ()
    --- A copy of this entity's `Scatter` component, or nil when it has none.
    function get(self, component: "Scatter"): Scatter?
    --- Patches this entity's `Scatter` component with the given fields.
    function set(self, component: "Scatter", value: ScatterPatch): ()
    --- A copy of this entity's `Script` component, or nil when it has none.
    function get(self, component: "Script"): Script?
    --- Patches this entity's `Script` component with the given fields.
    function set(self, component: "Script", value: ScriptPatch): ()
    --- A copy of this entity's `ScriptIntent` component, or nil when it has none.
    function get(self, component: "ScriptIntent"): ScriptIntent?
    --- Patches this entity's `ScriptIntent` component with the given fields.
    function set(self, component: "ScriptIntent", value: ScriptIntentPatch): ()
    --- A copy of this entity's `ShaderMaterial` component, or nil when it has none.
    function get(self, component: "ShaderMaterial"): ShaderMaterial?
    --- Patches this entity's `ShaderMaterial` component with the given fields.
    function set(self, component: "ShaderMaterial", value: ShaderMaterialPatch): ()
    --- A copy of this entity's `SkinBinding` component, or nil when it has none.
    function get(self, component: "SkinBinding"): SkinBinding?
    --- Patches this entity's `SkinBinding` component with the given fields.
    function set(self, component: "SkinBinding", value: SkinBindingPatch): ()
    --- A copy of this entity's `Spawner` component, or nil when it has none.
    function get(self, component: "Spawner"): Spawner?
    --- Patches this entity's `Spawner` component with the given fields.
    function set(self, component: "Spawner", value: SpawnerPatch): ()
    --- A copy of this entity's `Spinner` component, or nil when it has none.
    function get(self, component: "Spinner"): Spinner?
    --- Patches this entity's `Spinner` component with the given fields.
    function set(self, component: "Spinner", value: SpinnerPatch): ()
    --- A copy of this entity's `Spline` component, or nil when it has none.
    function get(self, component: "Spline"): Spline?
    --- Patches this entity's `Spline` component with the given fields.
    function set(self, component: "Spline", value: SplinePatch): ()
    --- A copy of this entity's `SpotLight` component, or nil when it has none.
    function get(self, component: "SpotLight"): SpotLight?
    --- Patches this entity's `SpotLight` component with the given fields.
    function set(self, component: "SpotLight", value: SpotLightPatch): ()
    --- A copy of this entity's `StandardMaterial` component, or nil when it has none.
    function get(self, component: "StandardMaterial"): StandardMaterial?
    --- Patches this entity's `StandardMaterial` component with the given fields.
    function set(self, component: "StandardMaterial", value: StandardMaterialPatch): ()
    --- A copy of this entity's `Sweep` component, or nil when it has none.
    function get(self, component: "Sweep"): Sweep?
    --- Patches this entity's `Sweep` component with the given fields.
    function set(self, component: "Sweep", value: SweepPatch): ()
    --- A copy of this entity's `Terrain` component, or nil when it has none.
    function get(self, component: "Terrain"): Terrain?
    --- Patches this entity's `Terrain` component with the given fields.
    function set(self, component: "Terrain", value: TerrainPatch): ()
    --- A copy of this entity's `TerrainChunk` component, or nil when it has none.
    function get(self, component: "TerrainChunk"): TerrainChunk?
    --- Patches this entity's `TerrainChunk` component with the given fields.
    function set(self, component: "TerrainChunk", value: TerrainChunkPatch): ()
    --- A copy of this entity's `TerrainPath` component, or nil when it has none.
    function get(self, component: "TerrainPath"): TerrainPath?
    --- Patches this entity's `TerrainPath` component with the given fields.
    function set(self, component: "TerrainPath", value: TerrainPathPatch): ()
    --- A copy of this entity's `TerrainPathChunk` component, or nil when it has none.
    function get(self, component: "TerrainPathChunk"): TerrainPathChunk?
    --- Patches this entity's `TerrainPathChunk` component with the given fields.
    function set(self, component: "TerrainPathChunk", value: TerrainPathChunkPatch): ()
    --- A copy of this entity's `Transform` component, or nil when it has none.
    function get(self, component: "Transform"): Transform?
    --- Patches this entity's `Transform` component with the given fields.
    function set(self, component: "Transform", value: TransformPatch): ()
    --- A copy of this entity's `Transient` component, or nil when it has none.
    function get(self, component: "Transient"): Transient?
    --- Patches this entity's `Transient` component with the given fields.
    function set(self, component: "Transient", value: TransientPatch): ()
    --- A copy of this entity's `Uid` component, or nil when it has none.
    function get(self, component: "Uid"): Uid?
    --- Patches this entity's `Uid` component with the given fields.
    function set(self, component: "Uid", value: UidPatch): ()
    --- A copy of this entity's `Velocity` component, or nil when it has none.
    function get(self, component: "Velocity"): Velocity?
    --- Patches this entity's `Velocity` component with the given fields.
    function set(self, component: "Velocity", value: VelocityPatch): ()
    --- A copy of this entity's `Visible` component, or nil when it has none.
    function get(self, component: "Visible"): Visible?
    --- Patches this entity's `Visible` component with the given fields.
    function set(self, component: "Visible", value: VisiblePatch): ()
    --- A copy of a component (`e:get("Transform")`) or of one of its fields (`e:get("Transform.translation")`), or nil when the entity does not have the component. Always a copy, never a live view: change it with `set`. Unknown names fail with a did-you-mean.
    function get(self, path: string): any
    --- Patches a component or one field: `e:set("Transform", { translation = v })` changes only the fields given, `e:set("Transform.scale", v)` one field. Values are checked against the component's field types. The entity must have the component (`add` adds one).
    function set(self, path: string, value: any): ()
end

-- world
--- The game world: spawn and find entities, query them by component, read and change resources.
declare world: {
    --- Spawns an entity with a fresh uid and returns it. `spec` keys: `name` (a string), `parent` (an entity), `transient` (true: runtime-only, never saved, removed when a save loads) and any registered component name with a table of fields (defaults patched with them), such as `{ name = "Coin", Transform = { translation = p }, Primitive = { shape = { Sphere = {} } } }`. A default `Transform` is added unless one is given. Runtime entities are removed by a scene switch unless kept (`scenes.persist`).
    spawn: (spec: { [string]: any }?) -> Entity,
    --- The first entity whose `Name` is `name`, or nil. Names need not be unique.
    find: (name: string) -> Entity?,
    --- Every entity named `name`.
    find_all: (name: string) -> { Entity },
    --- The entity with this uid (as `e:uid()` returns it), or nil.
    entity: (uid: string) -> Entity?,
    --- Every entity that has all of the named components (given as separate arguments or one array), for example `world.query("Transform", "Collider")`. Loop the result and use entity methods: the batch form for many entities.
    query: (...(string | { string })) -> { Entity },
    --- How many entities have all of the named components (separate arguments or one array).
    count: (...(string | { string })) -> number,
    --- Calls `f(entity, a, b, ...)` for every entity with the named components, passing copies of their values: convenient for reading many entities (about 0.6 µs per row).
    each: (components: string | { string }, f: (Entity, ...any) -> ()) -> (),
    --- A copy of a registered resource (`world.resource("Environment")`) or one of its fields (`"Environment.exposure"`), or nil when the world has none.
    resource: ((name: "AudioSettings") -> AudioSettings?) & ((name: "Environment") -> Environment?) & ((name: "GameRules") -> GameRules?) & ((name: "GameState") -> GameState?) & ((name: "GameVars") -> GameVars?) & ((name: "HudSettings") -> HudSettings?) & ((name: "NavSettings") -> NavSettings?) & ((name: "PhysicsSettings") -> PhysicsSettings?) & ((name: string) -> any),
    --- Patches a resource, or one field (`world.set_resource("Environment.exposure", 1.5)`), with the given value.
    set_resource: (name: string, value: any) -> (),
    --- Removes an entity and its descendants (like `e:despawn()`). Returns false if it was already gone.
    despawn: (entity: Entity) -> boolean,
    --- The `self` tables of every running instance of the script at `path` (assets-relative, such as "scripts/enemy.luau"), in start order: script data in bulk without copying components.
    scripts: (path: string) -> { any },
}

-- time
--- Game time: the frame's delta, elapsed time, time scale and pause.
declare time: {
    --- Seconds of game time this frame (scaled; 0 while paused). Inside `on_fixed_update` it is the fixed step.
    delta: () -> number,
    --- Seconds of game time since the game started (scaled, stops while paused).
    elapsed: () -> number,
    --- The number of frames since the game started.
    frame: () -> number,
    --- The time scale: 1 is normal speed, 0.5 half speed.
    scale: () -> number,
    --- Sets the time scale (clamped at 0). Slows or speeds up `dt`, waits, physics and animation.
    set_scale: (scale: number) -> (),
    --- True while game time is paused.
    is_paused: () -> boolean,
    --- Deprecated: use `is_paused`.
    paused: () -> boolean,
    --- Pauses or resumes game time: `dt` becomes 0, fixed steps and waits stop, `on_update` still runs (for menus).
    set_paused: (paused: boolean) -> (),
    --- The fixed step in seconds (1/60 by default): the `dt` of `on_fixed_update`.
    fixed_step: () -> number,
}

-- app
--- The running application.
declare app: {
    --- Asks the player to exit after this frame. In the editor it stops Play.
    quit: () -> (),
}

-- input
--- Player input: named actions (keyboard, mouse and gamepad bindings), raw keys and mouse, cursor lock.
declare input: {
    --- True in the frame (or fixed step, inside `on_fixed_update`) where the action became held. Unknown action names read as not pressed and are logged once.
    pressed: (action: string) -> boolean,
    --- True while any binding of the action is held (an axis action: while it is not zero).
    held: (action: string) -> boolean,
    --- True in the frame (or fixed step) where the action stopped being held.
    released: (action: string) -> boolean,
    --- A one-axis action's value from -1 to 1 (a button action reads 1 while held).
    axis1: (action: string) -> number,
    --- A two-axis action (movement, look) as a vector with length at most 1: x is right, y is up or forward, z is 0.
    axis2: (action: string) -> vector,
    --- True while the key is held. Key names: "A" to "Z", "Digit0" to "Digit9", "Space", "Enter", "Escape", "Tab", "Up", "Down", "Left", "Right", "ShiftLeft", "ControlLeft", "F1" and so on.
    key_down: (key: string) -> boolean,
    --- True in the frame the key went down.
    key_pressed: (key: string) -> boolean,
    --- True in the frame the key went up.
    key_released: (key: string) -> boolean,
    --- True while the mouse button ("Left", "Right" or "Middle") is held.
    mouse_down: (button: string) -> boolean,
    --- True in the frame the mouse button went down.
    mouse_pressed: (button: string) -> boolean,
    --- True in the frame the mouse button was let go.
    mouse_released: (button: string) -> boolean,
    --- The raw mouse movement this frame (device units, x right, y down), for mouse look; it keeps reporting while the cursor is locked.
    mouse_delta: () -> vector,
    --- The cursor position in the window in physical pixels (from the top left; z is 0), or nil when the cursor is outside it.
    cursor_position: () -> vector?,
    --- Hides the cursor and locks it to the window (for mouse look).
    lock_cursor: () -> (),
    --- Shows and frees the cursor again.
    release_cursor: () -> (),
    --- True while the cursor is locked.
    is_cursor_locked: () -> boolean,
    --- Deprecated: use `is_cursor_locked`.
    cursor_locked: () -> boolean,
}

-- physics
--- Which colliders a query sees. By default every non-trigger collider on any layer.
export type QueryOptions = {
    --- An entity to ignore (usually the one asking, so a ray from inside its own collider does not hit it).
    exclude: Entity?,
    --- Collision layers to test, one bit per layer (`Collider.layers`); default all.
    mask: number?,
    --- Include trigger colliders (default false).
    triggers: boolean?,
}
--- A ray hit.
export type RayHit = {
    --- The entity whose collider was hit.
    entity: Entity,
    --- Distance from the ray's origin in metres.
    distance: number,
    --- The hit point in world space.
    point: vector,
    --- The surface normal at the hit point.
    normal: vector,
}
--- A shape cast hit.
export type ShapeHit = {
    --- The entity whose collider was hit.
    entity: Entity,
    --- How far the shape travelled before touching, in metres.
    distance: number,
    --- The contact point in world space.
    point: vector,
    --- The surface normal at the contact (when `penetrating`, the direction to push out along).
    normal: vector,
    --- The shape already overlapped the collider where it started (`distance` is 0).
    penetrating: boolean,
}
--- Options of `physics.move_and_slide`: the capsule's size, what blocks it and how it walks.
export type SlideOptions = {
    --- Capsule radius in metres (default 0.4).
    radius: number?,
    --- Capsule total height in metres (default 1.8).
    height: number?,
    --- An entity to ignore (the moving one's own collider).
    exclude: Entity?,
    --- Collision layers that block (`Collider.layers` bits); default all.
    mask: number?,
    --- Gap kept between the capsule and what it touches, in metres (default 0.02).
    skin: number?,
    --- Highest step climbed without jumping when `on_ground`, in metres (default 0).
    step_height: number?,
    --- After moving, look this far down for a floor and snap onto it, in metres (default 0.1).
    ground_snap: number?,
    --- The steepest walkable slope, in radians (default about 0.93, 53 degrees); steeper surfaces are walls.
    max_slope: number?,
    --- The capsule stood on the ground before this move; enables `step_height`.
    on_ground: boolean?,
}
--- Where `physics.move_and_slide` left the capsule.
export type SlideResult = {
    --- The new position (the capsule's centre).
    position: vector,
    --- A walkable floor is under the capsule after the move.
    grounded: boolean,
    --- The floor's normal when `grounded`, else zero.
    ground_normal: vector,
    --- The entity whose collider is the floor, when `grounded`.
    ground_entity: Entity?,
    --- Moving up, the capsule hit a ceiling.
    hit_ceiling: boolean,
    --- Every surface hit while sliding, in order.
    hits: { ShapeHit },
}

--- Spatial queries against colliders, kinematic movement, and forces on rigid bodies.
declare physics: {
    --- The nearest collider hit by a ray from `origin` along `direction` (normalized for you) within `max_distance` metres (default 1000), or nil.
    raycast: (origin: vector, direction: vector, max_distance: number?, options: QueryOptions?) -> RayHit?,
    --- Every collider hit by the ray, nearest first (one hit per collider).
    raycast_all: (origin: vector, direction: vector, max_distance: number?, options: QueryOptions?) -> { RayHit },
    --- Sweeps a sphere of `radius` from `origin` along `direction` and returns the first collider it touches within `max_distance` (default 1000), or nil.
    spherecast: (origin: vector, radius: number, direction: vector, max_distance: number?, options: QueryOptions?) -> ShapeHit?,
    --- Every collider overlapping a sphere.
    overlap_sphere: (center: vector, radius: number, options: QueryOptions?) -> { Entity },
    --- Every collider overlapping a box with edge lengths `size`, centred at `center` and turned by `rotation`.
    overlap_box: (center: vector, size: vector, rotation: Quat?, options: QueryOptions?) -> { Entity },
    --- Every collider containing `point` (solid shapes: a point inside a box counts).
    contains_point: (point: vector, options: QueryOptions?) -> { Entity },
    --- Moves a capsule from `position` by `motion`, sliding along walls, climbing slopes and steps, and snapping to the ground; returns where it ended. For your own character controllers (a `KinematicCharacter` already does this every fixed step).
    move_and_slide: (position: vector, motion: vector, options: SlideOptions?) -> SlideResult,
    --- Changes a dynamic body's momentum by `impulse` (N·s, world space) through its centre of mass, at the next physics step: a kick, a jump, an explosion. Wakes the body.
    apply_impulse: (entity: Entity, impulse: vector) -> (),
    --- Like `apply_impulse`, at the world point `point`, which also spins the body.
    apply_impulse_at_point: (entity: Entity, impulse: vector, point: vector) -> (),
    --- Changes a dynamic body's angular momentum by `impulse` (N·m·s; world axis times magnitude) at the next step.
    apply_torque_impulse: (entity: Entity, impulse: vector) -> (),
    --- Pushes a dynamic body with `force` (N, world space) during the next step only: call it every fixed step for a continuous force (thrust, wind).
    add_force: (entity: Entity, force: vector) -> (),
    --- Turns a dynamic body with `torque` (N·m) during the next step only.
    add_torque: (entity: Entity, torque: vector) -> (),
    --- A body's linear and angular velocity (m/s, rad/s) as of the last step, or nil, nil when it has none.
    velocity: (entity: Entity) -> (vector?, vector?),
    --- Whether a body is asleep (at rest and not simulated), or nil when it has no body.
    is_sleeping: (entity: Entity) -> boolean?,
    --- The angle of the entity's hinge `Joint` (radians from its rest pose) or the travel of its prismatic joint (metres), or nil.
    joint_position: (entity: Entity) -> number?,
}

-- character
--- Kinematic characters: entities with a `KinematicCharacter` component, moved by the engine every fixed step.
declare character: {
    --- Sets the horizontal velocity (x and z of `velocity`, m/s); gravity keeps the vertical part. Call it every fixed step from `on_fixed_update`.
    move: (entity: Entity, velocity: vector) -> (),
    --- Sets the whole velocity, vertical part included (a launch pad, a knockback).
    set_velocity: (entity: Entity, velocity: vector) -> (),
    --- The current velocity in m/s.
    velocity: (entity: Entity) -> vector,
    --- Jumps with the character's `jump_speed` if it stands on the ground. Returns true if it jumped.
    jump: (entity: Entity) -> boolean,
    --- True while the character stands on a walkable floor (as the last step left it).
    is_grounded: (entity: Entity) -> boolean,
    --- The entity the character stands on, or nil (a moving platform carries the character).
    ground_entity: (entity: Entity) -> Entity?,
    --- The floor's normal while grounded, else zero.
    ground_normal: (entity: Entity) -> vector,
    --- Moves the character to `position` at once: stops it and resets interpolation, so it does not slide there visibly.
    teleport: (entity: Entity, position: vector) -> (),
}

-- events
--- Events between scripts and the engine: registered event types and script-only signals.
declare events: {
    --- Calls `handler(event)` for every event or signal named `name`, in the scripts' Update stage before `on_update`. Registered event types arrive as copies (sent since the handler was added); signals with their payload. Returns an id for `events.off`. The handler belongs to the calling script instance and stops with it.
    on: (name: string, handler: (event: any) -> ()) -> number,
    --- Removes a handler. Returns true if it existed.
    off: (id: number) -> boolean,
    --- Sends an event. A registered event type (`"PlaySound"`) is built from its default patched with `payload` and sent on its queue, where native systems and script handlers read it; any other name is a script signal, delivered to `events.on` handlers with `payload` as is (a signal sent during `on_update` arrives next frame).
    send: ((name: "Interacted", event: InteractedPatch?) -> ()) & ((name: "PlaySound", event: PlaySoundPatch?) -> ()) & ((name: string, payload: any?) -> ()),
}

-- ground
--- The ground at a point.
export type GroundHit = {
    --- Height of the ground in metres.
    height: number,
    --- The surface normal (unit length, pointing up out of the ground).
    normal: vector,
    --- The point on the ground: `x`, `height`, `z`.
    position: vector,
}
--- The terrain under a point: heights of the drawn and walked surface of every `Terrain` (the highest where they overlap). Also available in rules, where sampling it makes the rule re-run where the terrain changes.
declare ground: {
    --- The ground height at world `x`, `z` in metres, or nil where there is no terrain.
    height: (x: number, z: number) -> number?,
    --- Height, normal and point of the ground at world `x`, `z`, or nil where there is no terrain.
    at: (x: number, z: number) -> GroundHit?,
}

-- ui
--- A handle to a node of the game UI (returned by `ui.add`, `ui.find`, `ui.children`, events). Compares with `==`. A handle to a removed node makes functions fail; `ui.contains` checks.
declare extern type UiNode with
end

--- A node argument: a `UiNode`, or a node `id` string (looked up like `ui.find(id)`, failing when there is none).
export type UiTarget = UiNode | string
--- What happened to a node: a button clicked, a slider changed, a toggle toggled, focus gained or lost, or back pressed (Escape, Backspace, gamepad East) in a modal (the event is on the modal root).
export type UiEventKind = "click" | "change" | "toggle" | "focus" | "blur" | "back"
--- A UI event, as `ui.on` handlers receive it.
export type UiEvent = {
    --- The node the event happened on.
    node: UiNode,
    --- The node's `id`, if it has one.
    id: string?,
    --- The node's `action`, if it has one.
    action: string?,
    --- What happened.
    kind: UiEventKind,
    --- The new slider value (`change` events).
    value: number?,
    --- The new toggle state (`toggle` events).
    checked: boolean?,
}

--- The game UI: menus, HUDs and labels over the game, built from documents (JSON text or tables) and changed through setters. Every setter returns true if it changed something.
declare ui: {
    --- Adds a document (a node with its children) under `parent`, or as a new root, and returns its node. A document is a JSON string or a table with the same fields: `kind`, `id`, `text` or `key`, `args`, `class`, `style`, `children`, `anchor`, `modal` and so on (see the overview).
    add: (document: string | { [string]: any }, parent: UiTarget?) -> UiNode,
    --- The first node with this `id` (depth first) under `scope` (inclusive), or in the whole UI; nil when there is none.
    find: (id: string, scope: UiTarget?) -> UiNode?,
    --- Removes a node and its descendants. Returns false if it was not there.
    remove: (node: UiTarget) -> boolean,
    --- True if the node still exists.
    contains: (node: UiTarget) -> boolean,
    --- Shows literal text (and clears a string table key).
    set_text: (node: UiTarget, text: string) -> boolean,
    --- Shows the text of a string table key (in the current language; it follows language switches).
    set_key: (node: UiTarget, key: string) -> boolean,
    --- Sets the value of a `{name}` placeholder in the node's text.
    set_arg: (node: UiTarget, name: string, value: string | number | boolean) -> boolean,
    --- Sets several placeholder values at once.
    set_args: (node: UiTarget, args: { [string]: string | number | boolean }) -> boolean,
    --- Shows or hides the node (and its children).
    set_visible: (node: UiTarget, visible: boolean) -> boolean,
    --- Enables or disables an interactive node (a disabled button cannot be focused or clicked).
    set_enabled: (node: UiTarget, enabled: boolean) -> boolean,
    --- Sets a slider or progress value (clamped to its range). Sends no event.
    set_value: (node: UiTarget, value: number) -> boolean,
    --- Sets a slider or progress range.
    set_range: (node: UiTarget, min: number, max: number) -> boolean,
    --- Sets a toggle. Sends no event.
    set_checked: (node: UiTarget, checked: boolean) -> boolean,
    --- Replaces the node's extra theme classes (space separated).
    set_class: (node: UiTarget, class: string) -> boolean,
    --- Replaces the node's inline style (a JSON string or a table: `background`, `text_color`, `text_size`, `padding`, `gap`, `radius`, `width`, `height` and the other theme style fields).
    set_style: (node: UiTarget, style: string | { [string]: any }) -> boolean,
    --- Shows another image asset (assets-relative path) in an image node.
    set_image: (node: UiTarget, path: string) -> boolean,
    --- Sets (or clears, with nil) the action name sent with the node's events.
    set_action: (node: UiTarget, action: string?) -> boolean,
    --- Places a root node on the screen: `anchor` from {0, 0} (top left) to {1, 1} (bottom right), also its pivot; `offset` in reference points.
    set_anchor: (node: UiTarget, anchor: { number } | vector, offset: ({ number } | vector)?) -> boolean,
    --- Makes a root node follow an entity's position plus `offset` (metres), such as a name tag or a health bar. It is hidden while the point is off screen.
    anchor_to_entity: (node: UiTarget, entity: Entity, offset: vector?) -> boolean,
    --- Makes a root node follow a world point.
    anchor_to_point: (node: UiTarget, point: vector) -> boolean,
    --- Puts a world-anchored root node back at its screen anchor.
    clear_world_anchor: (node: UiTarget) -> boolean,
    --- Moves keyboard and gamepad focus to a focusable node (sends `blur` and `focus` events).
    focus: (node: UiTarget) -> boolean,
    --- Removes focus from every node.
    clear_focus: () -> (),
    --- The focused node, or nil.
    focused: () -> UiNode?,
    --- The visible modal root that owns focus and captures game input, or nil.
    active_modal: () -> UiNode?,
    --- True while a modal root captures game input (gameplay actions read as idle).
    captures_input: () -> boolean,
    --- The node's text as last laid out (string table and placeholders applied, inline tags removed), or nil.
    text: (node: UiTarget) -> string?,
    --- A slider or progress value, or nil for other kinds.
    value: (node: UiTarget) -> number?,
    --- A toggle's state, or nil for other kinds.
    is_checked: (node: UiTarget) -> boolean?,
    --- Deprecated: use `is_checked`.
    checked: (node: UiTarget) -> boolean?,
    --- True unless the node itself is hidden (`set_visible(false)`).
    is_visible: (node: UiTarget) -> boolean,
    --- True if the node was drawn in the last layout (it and every ancestor visible, a world anchor on screen).
    is_shown: (node: UiTarget) -> boolean,
    --- True unless the node is disabled.
    is_enabled: (node: UiTarget) -> boolean,
    --- The node's kind ("panel", "column", "row", "stack", "text", "image", "button", "slider", "toggle", "list", "progress" or "spacer").
    kind: (node: UiTarget) -> string?,
    --- The node's `id`, or nil.
    id: (node: UiTarget) -> string?,
    --- The node's rectangle from the last layout in physical pixels (x, y from the top left; w, h), or nil before its first layout.
    rect: (node: UiTarget) -> { x: number, y: number, w: number, h: number }?,
    --- The parent node, or nil for a root.
    parent: (node: UiTarget) -> UiNode?,
    --- The child nodes, in order.
    children: (node: UiTarget) -> { UiNode },
    --- Uses a theme file (`*.theme.json`, assets-relative), merged over the built-in theme and hot-reloaded.
    set_theme: (path: string) -> (),
    --- Actions that still reach the game while a modal captures input (a "pause" action that also closes the pause menu).
    set_passthrough_actions: (actions: { string }) -> (),
    --- Calls `handler(event)` for UI events of `kind` ("any" for all kinds), only for the node with `id` when given (`ui.on(kind, handler)` or `ui.on(kind, id, handler)`). Handlers run in the scripts' Update stage, belong to the calling script instance and stop with it. Returns an id for `ui.off`.
    on: ((kind: UiEventKind | "any", handler: (event: UiEvent) -> ()) -> number) & ((kind: UiEventKind | "any", id: string?, handler: (event: UiEvent) -> ()) -> number),
    --- Removes a handler added with `ui.on`. Returns true if it existed.
    off: (handler: number) -> boolean,
}

-- strings
--- Localized text from the project's string tables.
declare strings: {
    --- The text for `key` (dotted, such as "menu.resume") in the active language, else the fallback language, else the key itself (a missing key is warned about once).
    get: (key: string) -> string,
    --- True if `key` has text in the active or fallback language.
    has: (key: string) -> boolean,
    --- The text for `key` with `{name}` placeholders filled from `args`.
    format: (key: string, args: { [string]: string | number | boolean }?) -> string,
    --- The active language code, such as "en".
    language: () -> string,
    --- Switches the language; its tables load on the next frame and UI text shown with `key` updates.
    set_language: (language: string) -> (),
}

-- scenes
--- Runtime scenes: switch levels, load and unload scenes additively.
declare scenes: {
    --- Replaces every loaded scene with `scene` (a name such as "level2", for `assets/scenes/level2.scene.json`, or an assets-relative path) at the start of the next Update. The file is validated first; on failure nothing changes. Returns the id the new scene instance will have.
    switch: (scene: string) -> number,
    --- Loads `scene` next to the loaded ones (a room, a UI scene) at the start of the next Update. Returns its instance id for `scenes.unload`.
    load_additive: (scene: string) -> number,
    --- Deprecated: use `load_additive`.
    load: (scene: string) -> number,
    --- Removes a loaded scene instance and its entities.
    unload: (id: number) -> (),
    --- The path of the main scene (the last switch, or the start scene), or nil.
    current: () -> string?,
    --- True while the scene instance `id` is loaded.
    is_loaded: (id: number) -> boolean,
    --- Marks a runtime entity (spawned by a script, not loaded from a scene) to survive scene switches (`keep` false unmarks it). Returns false if the entity is gone.
    persist: (entity: Entity, keep: boolean?) -> boolean,
    --- What a scene switch does with runtime entities: "keep_all" (the default: they all survive) or "despawn_unless_persistent" (recommended: only root entities marked with `scenes.persist` survive).
    set_runtime_entities: (mode: "keep_all" | "despawn_unless_persistent") -> (),
}

-- animation
--- One clip playing in a blend (during a crossfade there are two).
export type AnimationLayer = {
    --- The clip's asset path.
    clip: string,
    --- The playback time in seconds.
    time: number,
    --- The blend weight from 0 to 1.
    weight: number,
}

--- Skeletal animation: state machine parameters, direct clip playback, playback state and joints.
declare animation: {
    --- Sets a number parameter of the entity's `AnimationStateMachine` (transitions test it, such as "speed" > 0.1).
    set_float: (entity: Entity, parameter: string, value: number) -> (),
    --- Sets a boolean parameter of the state machine.
    set_bool: (entity: Entity, parameter: string, value: boolean) -> (),
    --- Sets a trigger parameter: it stays set until a transition that tests it fires (and clears it).
    trigger: (entity: Entity, parameter: string) -> (),
    --- A number parameter's value (0 when unset).
    float: (entity: Entity, parameter: string) -> number,
    --- A boolean or trigger parameter's value (false when unset).
    bool: (entity: Entity, parameter: string) -> boolean,
    --- The state machine's current state name, or nil before its first frame.
    state: (entity: Entity) -> string?,
    --- How many transitions the state machine has taken (to notice a change of state).
    transitions: (entity: Entity) -> number,
    --- Plays a clip (asset path, such as "models/hero.glb#Animation/Run") on the entity's `AnimationPlayer`, crossfading over `crossfade` seconds (default the player's own crossfade).
    play: (entity: Entity, clip: string, crossfade: number?) -> (),
    --- The current clip's playback time in seconds.
    time: (entity: Entity) -> number,
    --- The current clip's playback position from 0 to 1.
    normalized_time: (entity: Entity) -> number,
    --- True when a non-looping clip has reached its end.
    is_finished: (entity: Entity) -> boolean,
    --- Deprecated: use `is_finished`.
    finished: (entity: Entity) -> boolean,
    --- The clips playing now with their times and blend weights.
    layers: (entity: Entity) -> { AnimationLayer },
    --- The first descendant named `name` (a joint of the skeleton, breadth first), or nil: attach a sword to a hand by parenting it to the joint.
    joint: (entity: Entity, name: string) -> Entity?,
}

-- save
--- A save file, as `save.slots` lists it.
export type SaveSlot = {
    --- The slot name.
    slot: string,
    --- When it was written, in Unix seconds.
    saved_at: number,
    --- The game's save version when it was written.
    version: number,
    --- The main scene's path when it was saved.
    scene: string?,
}
--- The outcome of a save or load request, as `save.on` handlers receive it.
export type SaveResult = {
    --- What happened.
    kind: "saved" | "loaded" | "failed",
    --- The slot of the request.
    slot: string,
    --- The save file's path.
    path: string?,
    --- How many entities were saved or loaded.
    entities: number?,
    --- Why it failed (`failed` only).
    message: string?,
}

--- Save games: snapshots of the running world in named slots.
declare save: {
    --- Saves the game to `slot` (letters, digits, `-` and `_`, at most 64) at the start of the next Update. The result arrives at `save.on` handlers.
    save: (slot: string) -> (),
    --- Loads `slot` at the start of the next Update: the world is replaced by the saved one and scripts get their `on_load`. The result arrives at `save.on` handlers.
    load: (slot: string) -> (),
    --- The save files, newest first.
    slots: () -> { SaveSlot },
    --- Makes a runtime entity (spawned by a script) part of save games (`Saveable`); `included = false` takes that back. Entities loaded from scenes are saved anyway. Returns false if the entity is gone.
    include: (entity: Entity, included: boolean?) -> boolean,
    --- Deprecated: use `include`.
    keep: (entity: Entity) -> boolean,
    --- Leaves an entity and its descendants out of save games (`excluded` false puts it back). Returns false if the entity is gone.
    exclude: (entity: Entity, excluded: boolean?) -> boolean,
    --- Calls `handler(result)` after each save or load request finishes. Returns an id for `events.off`. The handler belongs to the calling script instance.
    on: (handler: (result: SaveResult) -> ()) -> number,
    --- Removes a `save.on` handler by the id it returned.
    off: (id: number) -> boolean,
}

-- data
--- Data tables: JSON files (`*.data.json`) as read-only Luau tables.
declare data: {
    --- The data table at `path` (assets-relative, such as "data/enemies.data.json") as a read-only Luau table, or nil until the file is imported (the first call requests it, so check for nil in the first frames). The same table is returned until the file changes; copy what you want to change.
    table: (path: string) -> any,
    --- The table's version: it changes on every hot reload, so a script can rebuild what it derived from the table only when needed. Nil until imported.
    version: (path: string) -> number?,
}

-- particles
--- Particle effects: one-shot bursts and control of `ParticleEmitter` entities.
declare particles: {
    --- Spawns a one-shot burst of `count` particles at `position` (world space) and returns its emitter entity, which despawns itself when the last particle dies. `options` patches the `ParticleEmitter` fields (speed, colours, size, lifetime, gravity and so on).
    burst: (position: vector, count: number, options: ParticleEmitterPatch?) -> Entity,
    --- Starts (or resumes) emitting.
    start: (emitter: Entity) -> (),
    --- Stops emitting; live particles finish their lives.
    stop: (emitter: Entity) -> (),
    --- Clears the emitter's particles and starts it again from the beginning (bursts included).
    restart: (emitter: Entity) -> (),
    --- How many particles are alive.
    count: (emitter: Entity) -> number,
    --- True when a non-looping emitter has finished emitting and every particle has died.
    is_finished: (emitter: Entity) -> boolean,
}

-- camera
--- The active camera's projection: world points to screen pixels and screen points to world rays (picking).
declare camera: {
    --- Where world point `position` appears through the active camera: x and y in physical pixels from the top left (as `input.cursor_position()`), z its distance in front of the camera in metres. Nil when it is behind the camera or there is no active camera. A point beside the view is returned with x or y outside `camera.screen_size()`.
    world_to_screen: (position: vector) -> vector?,
    --- The world-space ray from the active camera through screen point `position` (physical pixels from the top left; z is ignored): its origin and normalized direction, or nil, nil without an active camera. Cast it with `physics.raycast`.
    screen_ray: (position: vector) -> (vector?, vector?),
    --- The ray under the mouse cursor (`camera.screen_ray(input.cursor_position())`), or nil, nil when the cursor is outside the window.
    cursor_ray: () -> (vector?, vector?),
    --- The size of the game view in physical pixels (z is 0).
    screen_size: () -> vector,
}

-- vars
--- A change of a game variable, as `vars.on_change` handlers receive it.
export type VarChange = {
    --- The variable's name.
    name: string,
    --- The new value.
    value: number,
    --- The value before the change.
    old: number,
}
--- Game variables: named numbers for puzzle and progress state (`embers`, `gate_open`), shared with `Interactable` conditions and actions, kept across scene switches and saved in save games. Unset names read 0.
declare vars: {
    --- The value of `name` (0 when unset).
    get: (name: string) -> number,
    --- Sets `name` to `value`.
    set: (name: string, value: number) -> (),
    --- Adds `amount` (negative: subtracts) to `name` and returns the new value.
    add: (name: string, amount: number) -> number,
    --- Every variable that was set, by name.
    all: () -> { [string]: number },
    --- Calls `handler` in the next Update after `name` changes (by a script, an `Interactable` or a loaded save). Returns an id for `vars.off`; removed with the script.
    on_change: (name: string, handler: (change: VarChange) -> ()) -> number,
    --- Removes an `on_change` handler.
    off: (id: number) -> (),
}

-- interact
--- Interactables (`Interactable` components): what the player can use, and using them from scripts.
declare interact: {
    --- The interactable whose prompt shows now (the nearest one in range), or nil.
    focus: () -> Entity?,
    --- Uses `entity`'s `Interactable` in the next Update as if the player pressed its action: its conditions, cooldown and `once` apply, its distance does not.
    use: (entity: Entity) -> (),
    --- True when `entity`'s `Interactable` is enabled, not used up and its conditions hold.
    can_use: (entity: Entity) -> boolean,
}

-- nav
--- Where characters the game moves can walk: the navmesh, built from the scene's static colliders.
declare nav: {
    --- True once the navmesh is built and no part of it is being rebuilt (right after a scene starts, or after colliders change, it is not: paths may be missing or change soon).
    ready: () -> boolean,
    --- The corners of the shortest walkable path from `from` to `to` (feet positions), start first; nil when `to` cannot be reached or there is no navmesh yet.
    path: (from: vector, to: vector) -> { vector }?,
    --- Whether something can walk from `from` to `to`.
    reachable: (from: vector, to: vector) -> boolean,
    --- The nearest walkable point to `position` (within 2 m sideways and 3 m up or down), or nil.
    closest: (position: vector) -> vector?,
    --- A random walkable point within `radius` metres of `center`, reachable from it (wander targets, spawn spots). The same `seed` gives the same point; without one it changes every frame.
    random_near: (center: vector, radius: number, seed: number?) -> vector?,
}

-- Script modules
--- One declared script property: what each entry of a module's `properties` table takes. Every field is optional. A bare value (`speed = 4`, `enabled = true`, `tint = Color.rgb(1, 0, 0)`) is shorthand for a declaration with only a `default`.
export type PropertyDecl = {
    --- The property type. Omit it to infer it from `default`; a number infers "number", so write "integer" for whole numbers.
    type: ("number" | "integer" | "boolean" | "string" | "vector" | "color" | "entity" | "asset")?,
    --- The default value, used until the inspector or the scene overrides it. Entity properties default to none; asset properties take an assets-relative path.
    default: any?,
    --- The smallest value the inspector allows (numbers and integers).
    min: number?,
    --- The largest value the inspector allows (numbers and integers).
    max: number?,
    --- The inspector's drag step.
    step: number?,
    --- A unit shown next to the value in the inspector, such as "m" or "deg/s".
    unit: string?,
    --- The inspector tooltip.
    doc: string?,
}

--- The `self` table of a running script instance (one per entity with a `Script`). Fields the script sets on it are the instance's runtime state. Its metatable is the module, so module functions can be called as methods (`self:helper()`).
export type ScriptSelf = {
    --- The entity the script is attached to.
    entity: Entity,
    --- The effective property values: declared defaults overridden by the entity's `Script` component. Entity properties arrive as entity handles (or nil), asset properties as paths. Refreshed on hot reload and inspector edits.
    props: { [string]: any },
}

--- The table a gameplay script file returns. Every field is optional; the runtime calls the callbacks the module defines. Instances run in scene order, so a scene plays the same way every time.
export type ScriptModule = {
    --- Declared properties: name to declaration (or to a bare default value). They appear in the inspector in declaration order and are saved with the scene.
    properties: { [string]: PropertyDecl | number | boolean | string | vector | Color }?,
    --- Runs once when the instance starts (when the entity has its `Script` and the file has loaded), as a task: it may `wait`.
    on_start: ((self: ScriptSelf) -> ())?,
    --- Runs every frame in Update, after the game's native Update systems. `dt` is the frame's game time in seconds (scaled by `time.scale`, 0 while paused).
    on_update: ((self: ScriptSelf, dt: number) -> ())?,
    --- Runs every fixed step (60 Hz by default) in FixedUpdate, before characters move, so `character.move` and `character.jump` apply in the same step. `dt` is the fixed step.
    on_fixed_update: ((self: ScriptSelf, dt: number) -> ())?,
    --- Runs after the entity or its `Script` component is gone; its components can no longer be read.
    on_destroy: ((self: ScriptSelf) -> ())?,
    --- Called when another collider starts overlapping this entity's trigger collider, or this entity enters another's trigger. Both entities are told.
    on_trigger_enter: ((self: ScriptSelf, other: Entity) -> ())?,
    --- Called when an overlap reported by `on_trigger_enter` ends.
    on_trigger_exit: ((self: ScriptSelf, other: Entity) -> ())?,
    --- Called when the player uses this entity's `Interactable` (after its built-in actions ran), with the entity that used it. Runs in the next Update.
    on_interact: ((self: ScriptSelf, interactor: Entity) -> ())?,
    --- Called when this rigid body or character starts touching `other`; `impulse` is the contact impulse in newton-seconds. Needs the engine's `dynamics` feature (on in the prebuilt editor and player).
    on_collision_enter: ((self: ScriptSelf, other: Entity, impulse: number) -> ())?,
    --- Called when a contact reported by `on_collision_enter` ends. Needs the `dynamics` feature.
    on_collision_exit: ((self: ScriptSelf, other: Entity) -> ())?,
    --- Returns the instance's runtime state as plain data (numbers, strings, booleans, vectors, Quats, entities and tables of them) for a save game. Properties are saved with the scene and need no hook.
    on_save: ((self: ScriptSelf) -> any)?,
    --- Restores what `on_save` returned when a save game loads. Runs after the respawned instance's `on_start` has reached its first wait, so set defaults in `on_start` and restore them here.
    on_load: ((self: ScriptSelf, data: any) -> ())?,
    --- Called on each instance when the file is saved and reloaded during Play. When the module defines it, the instance's tasks and handlers are cancelled first, so restart them here.
    on_reload: ((self: ScriptSelf) -> ())?,
}

-- Script rules
--- One intent a rule turns into parts: an entity with a `ScriptIntent` naming the rule file.
export type RuleIntent = {
    --- The owner entity's uid (part keys and warnings use it).
    uid: string,
    --- The intent's points in the owner's local space: a path, an outline or positions (the rule decides what they mean). Drawn and edited with the editor's Draw Path tool.
    points: { vector },
    --- The effective property values: the file's declared defaults overridden by the intent. Entities arrive as uid strings, assets as paths.
    props: { [string]: any },
    --- The owner's world position.
    position: vector,
    --- The owner's world rotation.
    rotation: Quat,
    --- The owner's world scale.
    scale: vector,
    --- A point in the owner's local space (such as one of `points`) in world space.
    to_world: (self: RuleIntent, p: vector) -> vector,
    --- A world-space point in the owner's local space.
    to_local: (self: RuleIntent, p: vector) -> vector,
}
--- The components a part gets, by name: each a patch over the component's default values. Any registered component except `Uid`, `Name`, `Derived`, `Parent`, `Children` and `GlobalTransform`.
export type RuleComponents = {
    --- A `AmbientVolume` component on the part (its default values patched with these fields).
    AmbientVolume: AmbientVolumePatch?,
    --- A `AnimationPlayer` component on the part (its default values patched with these fields).
    AnimationPlayer: AnimationPlayerPatch?,
    --- A `AnimationStateMachine` component on the part (its default values patched with these fields).
    AnimationStateMachine: AnimationStateMachinePatch?,
    --- A `AudioListener` component on the part (its default values patched with these fields).
    AudioListener: AudioListenerPatch?,
    --- A `AudioSource` component on the part (its default values patched with these fields).
    AudioSource: AudioSourcePatch?,
    --- A `Bobber` component on the part (its default values patched with these fields).
    Bobber: BobberPatch?,
    --- A `Camera` component on the part (its default values patched with these fields).
    Camera: CameraPatch?,
    --- A `Chaser` component on the part (its default values patched with these fields).
    Chaser: ChaserPatch?,
    --- A `Collidable` component on the part (its default values patched with these fields).
    Collidable: CollidablePatch?,
    --- A `Collider` component on the part (its default values patched with these fields).
    Collider: ColliderPatch?,
    --- A `DirectionalLight` component on the part (its default values patched with these fields).
    DirectionalLight: DirectionalLightPatch?,
    --- A `Extrude` component on the part (its default values patched with these fields).
    Extrude: ExtrudePatch?,
    --- A `FollowCamera` component on the part (its default values patched with these fields).
    FollowCamera: FollowCameraPatch?,
    --- A `FollowGround` component on the part (its default values patched with these fields).
    FollowGround: FollowGroundPatch?,
    --- A `Health` component on the part (its default values patched with these fields).
    Health: HealthPatch?,
    --- A `Interactable` component on the part (its default values patched with these fields).
    Interactable: InteractablePatch?,
    --- A `Interactor` component on the part (its default values patched with these fields).
    Interactor: InteractorPatch?,
    --- A `Interpolated` component on the part (its default values patched with these fields).
    Interpolated: InterpolatedPatch?,
    --- A `Joint` component on the part (its default values patched with these fields).
    Joint: JointPatch?,
    --- A `KinematicCharacter` component on the part (its default values patched with these fields).
    KinematicCharacter: KinematicCharacterPatch?,
    --- A `Lathe` component on the part (its default values patched with these fields).
    Lathe: LathePatch?,
    --- A `MaterialRef` component on the part (its default values patched with these fields).
    MaterialRef: MaterialRefPatch?,
    --- A `MeshAsset` component on the part (its default values patched with these fields).
    MeshAsset: MeshAssetPatch?,
    --- A `Mover` component on the part (its default values patched with these fields).
    Mover: MoverPatch?,
    --- A `NavAgent` component on the part (its default values patched with these fields).
    NavAgent: NavAgentPatch?,
    --- A `Overrides` component on the part (its default values patched with these fields).
    Overrides: OverridesPatch?,
    --- A `ParticleEmitter` component on the part (its default values patched with these fields).
    ParticleEmitter: ParticleEmitterPatch?,
    --- A `PlayerController` component on the part (its default values patched with these fields).
    PlayerController: PlayerControllerPatch?,
    --- A `PointLight` component on the part (its default values patched with these fields).
    PointLight: PointLightPatch?,
    --- A `PrefabInstance` component on the part (its default values patched with these fields).
    PrefabInstance: PrefabInstancePatch?,
    --- A `Primitive` component on the part (its default values patched with these fields).
    Primitive: PrimitivePatch?,
    --- A `Procedural` component on the part (its default values patched with these fields).
    Procedural: ProceduralPatch?,
    --- A `RigidBody` component on the part (its default values patched with these fields).
    RigidBody: RigidBodyPatch?,
    --- A `Scatter` component on the part (its default values patched with these fields).
    Scatter: ScatterPatch?,
    --- A `Script` component on the part (its default values patched with these fields).
    Script: ScriptPatch?,
    --- A `ScriptIntent` component on the part (its default values patched with these fields).
    ScriptIntent: ScriptIntentPatch?,
    --- A `ShaderMaterial` component on the part (its default values patched with these fields).
    ShaderMaterial: ShaderMaterialPatch?,
    --- A `SkinBinding` component on the part (its default values patched with these fields).
    SkinBinding: SkinBindingPatch?,
    --- A `Spawner` component on the part (its default values patched with these fields).
    Spawner: SpawnerPatch?,
    --- A `Spinner` component on the part (its default values patched with these fields).
    Spinner: SpinnerPatch?,
    --- A `Spline` component on the part (its default values patched with these fields).
    Spline: SplinePatch?,
    --- A `SpotLight` component on the part (its default values patched with these fields).
    SpotLight: SpotLightPatch?,
    --- A `StandardMaterial` component on the part (its default values patched with these fields).
    StandardMaterial: StandardMaterialPatch?,
    --- A `Sweep` component on the part (its default values patched with these fields).
    Sweep: SweepPatch?,
    --- A `Terrain` component on the part (its default values patched with these fields).
    Terrain: TerrainPatch?,
    --- A `TerrainChunk` component on the part (its default values patched with these fields).
    TerrainChunk: TerrainChunkPatch?,
    --- A `TerrainPath` component on the part (its default values patched with these fields).
    TerrainPath: TerrainPathPatch?,
    --- A `TerrainPathChunk` component on the part (its default values patched with these fields).
    TerrainPathChunk: TerrainPathChunkPatch?,
    --- A `Transform` component on the part (its default values patched with these fields).
    Transform: TransformPatch?,
    --- A `Transient` component on the part (its default values patched with these fields).
    Transient: TransientPatch?,
    --- A `Velocity` component on the part (its default values patched with these fields).
    Velocity: VelocityPatch?,
    --- A `Visible` component on the part (its default values patched with these fields).
    Visible: VisiblePatch?,
}
--- Where a part goes, relative to its owner (each field optional).
export type RuleTransform = {
    --- Position in the owner's local space (default the origin).
    position: vector?,
    --- Rotation relative to the owner (default none).
    rotation: Quat?,
    --- Scale: a vector, or a number for a uniform scale (default 1).
    scale: (vector | number)?,
}
--- What a rule run produces: parts and warnings.
export type RuleOut = {
    --- Adds a part: a generated child entity of `owner` (an intent or its uid) with the given components, keyed by `role` (unique per owner, not a bare number: "post3", "rail_left"). Parts with the same key are updated in place across runs, and user overrides follow them.
    part: (self: RuleOut, owner: RuleIntent | string, role: string, components: RuleComponents, transform: RuleTransform?) -> (),
    --- Adds a warning to the owner (shown in the inspector and the Console, prefixed with the rule's file and line), or to one part when `role` is given.
    warn: (self: RuleOut, owner: RuleIntent | string, message: string, role: string?) -> (),
}
--- The table a rule file returns.
export type RuleModule = {
    --- Required: called with every intent naming the file (or those its `scope` covers), in uid order, and an output to add parts and warnings to.
    rule: (intents: { RuleIntent }, out: RuleOut) -> (),
    --- Declared properties, as for a gameplay script (`PropertyDecl`); intents override them in the inspector.
    properties: { [string]: PropertyDecl | number | boolean | string | vector | Color }?,
    --- How far a run must look: "file" (the default: every run sees every intent), "intent" (an intent's parts depend only on itself) or `{ neighbours = metres }` (on itself and intents within that distance). A narrower scope makes edits cheaper; it is a promise the engine cannot check.
    scope: ("file" | "intent" | { neighbours: number })?,
}
