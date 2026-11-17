-- Spawns a ring of orbs and moves them all from one script: the batch
-- form for many entities (scripting guide).
local util = require("./util")

local Swarm = {}

Swarm.properties = {
    count = { type = "integer", default = 24, min = 1, max = 500 },
    radius = { type = "number", default = 3, min = 0.5, max = 20, unit = "m" },
}

function Swarm.on_start(self)
    self.orbs = {}
    for i = 1, self.props.count do
        local orb = world.spawn({
            name = "Orb",
            parent = self.entity,
            Transform = { translation = util.ring(self.props.count, self.props.radius, i) },
            Primitive = { shape = { Sphere = { radius = 0.1 } } },
        })
        table.insert(self.orbs, orb)
    end
end

function Swarm.on_update(self, dt)
    local t = time.elapsed()
    for i, orb in self.orbs do
        local p = util.ring(self.props.count, self.props.radius, i)
        orb:set_position(p + vector.create(0, math.sin(t * 2 + i) * 0.3, 0))
    end
end

return Swarm
