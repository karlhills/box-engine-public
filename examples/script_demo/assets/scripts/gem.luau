-- A gem: turns slowly and is a trigger the player collects. Sends
-- `collected` (a script signal the rules script listens to) and goes away.

local Gem = {}

Gem.properties = {
    points = { type = "integer", default = 10, min = 0, max = 1000, doc = "Score for collecting it" },
    turn = { type = "number", default = 1.5, min = -10, max = 10, unit = "rad/s" },
}

function Gem.on_update(self, dt: number)
    self.entity:rotate_y(self.props.turn * dt)
end

function Gem.on_trigger_enter(self, other: Entity)
    if other:name() ~= "Player" then
        return
    end
    events.send("collected", { points = self.props.points, at = self.entity:world_position() })
    self.entity:despawn()
end

return Gem
