-- A gem that bursts into particles when touched (reference: particles).
local Gem = {}

function Gem.on_trigger_enter(self, other)
    particles.burst(self.entity:world_position(), 24, {
        speed = vector.create(2, 4, 0),          -- slowest and fastest (x, y)
        color = {
            { t = 0, color = Color.hex("#7fe3ff") },
            { t = 1, color = Color.rgb(1, 1, 1, 0) },
        },
    })
    self.entity:despawn()
end

return Gem
