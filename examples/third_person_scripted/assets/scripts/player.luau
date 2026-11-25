-- The player. Lives on the "Player" entity, which also has:
--   * KinematicCharacter: the character controller (gravity, jumping,
--     steps and slopes). `character.move` and `character.jump` drive it.
--   * Collider (a capsule on physics layer 2, the layer pickups listen to).
--   * Interpolated: moved at the fixed rate, drawn smoothly every frame.
-- Its child "Body" shows the skinned humanoid (a PrefabInstance of
-- models/character.glb) and runs the animation state machine in
-- animation/locomotion.animgraph.json (Idle, Walk, Run, Jump).
--
-- Input comes from named actions (assets/settings/input.actions.json):
-- `move` is WASD, the arrows, the d-pad or the left stick; `jump` is Space
-- or the gamepad's South button.

local util = require("./util")

local Player = {}

-- Properties appear in the inspector; each entity can set its own values.
Player.properties = {
    speed = { type = "number", default = 6, min = 0, max = 30, unit = "m/s", doc = "Speed at full input" },
    turn_rate = { type = "number", default = 12, min = 0, max = 50, unit = "rad/s", doc = "How fast the body turns toward the movement" },
    fall_limit = { type = "number", default = -20, min = -200, max = 0, unit = "m", doc = "Below this height the player is put back at the start" },
    camera = { type = "entity", doc = "The follow camera; movement is relative to its view" },
    body = { type = "entity", doc = "The animated model (the child with the AnimationStateMachine)" },
}

function Player.on_start(self)
    self.start = self.entity:position()
    self.facing = 0
end

-- The way the camera looks (camera.luau keeps its orbit angle in `yaw`).
local function view_yaw(camera: Entity?): number
    if camera == nil or not camera:is_alive() then
        return 0
    end
    local follow = camera:script()
    if follow ~= nil and follow.yaw ~= nil then
        return follow.yaw
    end
    return util.yaw_of(camera:forward())
end

-- Movement runs at the fixed physics rate (60 Hz), whatever the frame rate.
function Player.on_fixed_update(self, dt: number)
    local me = self.entity
    local wish = util.wish(input.axis2("move"), view_yaw(self.props.camera))
    character.move(me, wish * self.props.speed)
    if input.pressed("jump") and character.jump(me) and self.props.body ~= nil then
        animation.trigger(self.props.body, "jump")
    end
    if me:position().y < self.props.fall_limit then
        character.teleport(me, self.start)
    end
end

-- Every frame: feed the animation state machine and turn the body.
function Player.on_update(self, dt: number)
    local body = self.props.body
    if body == nil or not body:is_alive() then
        return
    end
    local v = character.velocity(self.entity)
    local flat = vector.create(v.x, 0, v.z)
    local speed = vector.magnitude(flat)
    animation.set_float(body, "speed", speed)
    animation.set_bool(body, "grounded", character.is_grounded(self.entity))
    if speed > 0.1 then
        self.facing = util.turn_towards(self.facing, util.yaw_of(flat), self.props.turn_rate * dt)
        body:set_rotation(Quat.from_rotation_y(self.facing))
    end
end

return Player
