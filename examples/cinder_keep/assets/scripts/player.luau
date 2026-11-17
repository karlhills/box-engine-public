-- The hero: the `move` action runs the kinematic character relative to the
-- camera, `jump` jumps, `attack` swings the sword (a sweep around the hero
-- that the wave script turns into kills and deflected embers). The body
-- plays assets/animation/hero.animgraph.json: locomotion blended from the
-- speed, plus attack, hit and jump clips. Falling into the lava gap (below
-- tuning.player.fall_limit) kills. Health lives in the game (game.luau),
-- which tells the hero when it is hit or dies.

local util = require("./util")

local Player = {}

Player.properties = {
    speed = { type = "number", default = 6, min = 0, max = 30, unit = "m/s", doc = "Speed at full input" },
    turn_rate = { type = "number", default = 12, min = 0, max = 50, unit = "rad/s", doc = "How fast the body turns toward the movement" },
    camera = { type = "entity", doc = "The follow camera; movement is relative to its view" },
    body = { type = "entity", doc = "The animated model (the child with the AnimationStateMachine)" },
}

function Player.on_start(self)
    self.facing = util.yaw_of(self.entity:forward())
    self.cooldown = 0
    self.swings = 0
    self.jumps = 0
    self.dead = false
    self.tuning = util.wait_for_tuning()
    events.on("player_hit", function(ev)
        if self.props.body ~= nil then
            animation.trigger(self.props.body, "hit")
        end
        particles.burst(self.entity:position() + vector.create(0, 1.1, 0), 16, {
            speed = vector.create(1.5, 3.5, 0),
            lifetime = vector.create(0.25, 0.5),
            gravity = vector.create(0, -6, 0),
            color = {
                { t = 0, color = Color.rgb(1.0, 0.2, 0.1, 1) },
                { t = 1, color = Color.rgb(0.6, 0.05, 0.02, 0) },
            },
            intensity = 4,
            blend = "Additive",
        })
    end)
    events.on("player_died", function(_)
        self.dead = true
    end)
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

function Player.on_fixed_update(self, dt: number)
    local me = self.entity
    if self.dead or self.tuning == nil then
        character.move(me, vector.create(0, 0, 0))
        return
    end
    local wish = util.wish(input.axis2("move"), view_yaw(self.props.camera))
    character.move(me, wish * self.props.speed)
    if input.pressed("jump") and character.jump(me) then
        self.jumps += 1
        if self.props.body ~= nil then
            animation.trigger(self.props.body, "jump")
        end
    end
    if me:position().y < self.tuning.player.fall_limit then
        self.dead = true
        log.info("fell into the lava")
        events.send("player_fell", { at = me:position() })
    end
end

-- A ring of sparks where the blade sweeps.
local function sweep_sparks(at: vector)
    particles.burst(at, 36, {
        speed = vector.create(5, 7, 0),
        direction = vector.create(0, 1, 0),
        cone_angle = 1.45,
        lifetime = vector.create(0.15, 0.3),
        gravity = vector.create(0, -4, 0),
        drag = 4,
        size = { { t = 0, value = 0.16 }, { t = 1, value = 0.04 } },
        color = {
            { t = 0, color = Color.rgb(1.0, 0.85, 0.5, 1) },
            { t = 1, color = Color.rgb(1.0, 0.35, 0.05, 0) },
        },
        intensity = 5,
        blend = "Additive",
    })
end

function Player.on_update(self, dt: number)
    local body = self.props.body
    local t = self.tuning
    if t == nil then
        return
    end
    self.cooldown -= dt
    if not self.dead and input.pressed("attack") and self.cooldown <= 0 then
        self.cooldown = t.player.attack_cooldown
        self.swings += 1
        if body ~= nil then
            animation.trigger(body, "attack")
        end
        task.delay(t.player.attack_delay, function()
            if self.dead then
                return
            end
            local at = self.entity:position()
            events.send("player_attack", { at = at, radius = t.player.attack_radius })
            sweep_sparks(at + vector.create(0, 0.9, 0))
        end)
    end
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

function Player.on_save(self)
    return { swings = self.swings, jumps = self.jumps, facing = self.facing }
end

function Player.on_load(self, saved)
    self.swings = saved.swings
    self.jumps = saved.jumps
    self.facing = saved.facing
end

return Player
