-- The player: the `move` action (WASD, arrows, d-pad, left stick) runs the
-- kinematic character relative to the camera and `jump` jumps. The skinned
-- body turns to face where it goes, and its animation state machine
-- (assets/animation/locomotion.animgraph.json) blends idle, walk, run and
-- jump from the speed, the ground and the jump trigger. Falling out of the
-- arena puts the player back at its start. Tunables come from the data
-- table assets/data/tuning.data.json (hot-reloaded).

local util = require("./util")

local Player = {}

Player.properties = {
    speed = { type = "number", default = 6, min = 0, max = 30, unit = "m/s", doc = "Speed at full input" },
    turn_rate = { type = "number", default = 12, min = 0, max = 50, unit = "rad/s", doc = "How fast the body turns toward the movement" },
    camera = { type = "entity", doc = "The follow camera; movement is relative to its view" },
    body = { type = "entity", doc = "The animated model (the child with the AnimationStateMachine)" },
}

local TUNING = "data/tuning.data.json"

function Player.on_start(self)
    self.start = self.entity:position()
    self.facing = 0
    self.jumps = 0
    self.tuned = nil
end

-- The camera's orbit yaw (kept by camera.luau), else its facing.
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

-- The character settings from the data table, applied when it changes.
local function tune(self)
    local version = data.version(TUNING)
    if version == nil or version == self.tuned then
        return
    end
    local t = data.table(TUNING).character
    self.entity:set("KinematicCharacter", { ground_snap = t.ground_probe, max_fall_speed = t.max_fall_speed })
    self.fall_limit = t.fall_limit
    self.tuned = version
end

function Player.on_fixed_update(self, dt: number)
    tune(self)
    local me = self.entity
    local wish = util.wish(input.axis2("move"), view_yaw(self.props.camera))
    character.move(me, wish * self.props.speed)
    if input.pressed("jump") and character.jump(me) then
        self.jumps += 1
        if self.props.body ~= nil then
            animation.trigger(self.props.body, "jump")
        end
    end
    if me:position().y < (self.fall_limit or -30) then
        log.info("fell out of the arena; back to the start")
        character.teleport(me, self.start)
    end
end

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

-- Saved games: the jump count and where to respawn.
function Player.on_save(self)
    return { jumps = self.jumps, start = self.start, facing = self.facing }
end

function Player.on_load(self, saved)
    self.jumps = saved.jumps
    self.start = saved.start
    self.facing = saved.facing
end

return Player
