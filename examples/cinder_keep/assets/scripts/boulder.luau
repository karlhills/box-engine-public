-- A boulder: hurts the hero when it hits them moving (a collision event
-- from the rigid-body simulation), and is removed once it has fallen off
-- the level.

local util = require("./util")

local Boulder = {}

Boulder.properties = {
    min_speed = { type = "number", default = 1.5, min = 0, max = 20, unit = "m/s", doc = "Slower boulders do not hurt" },
}

function Boulder.on_start(self)
    self.hit_at = -10
    self.hits = 0
end

function Boulder.on_collision_enter(self, other: Entity, impulse: number)
    if not util.is_player(other) then
        return
    end
    local linear = physics.velocity(self.entity)
    if linear == nil or vector.magnitude(linear) < self.props.min_speed then
        return
    end
    if time.elapsed() - self.hit_at < 1 then
        return
    end
    self.hit_at = time.elapsed()
    self.hits += 1
    local t = util.tuning()
    events.send("damage", { amount = if t then t.boulder.damage else 20 })
end

function Boulder.on_update(self, dt: number)
    if self.entity:position().y < -12 then
        self.entity:despawn()
    end
end

return Boulder
