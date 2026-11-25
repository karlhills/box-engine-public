-- Bobber, as a script: floats the entity up and down around where it
-- started. Made by Convert to Script from the Bobber behaviour; each entity
-- keeps its own values in the Script's properties. Change anything: this
-- file is yours now.

local Bobber = {}

Bobber.properties = {
    enabled = { type = "boolean", default = true, doc = "Off: it stays where it is" },
    height = { type = "number", default = 0.15, min = 0, max = 10, unit = "m", doc = "How far above and below its start it goes" },
    period = { type = "number", default = 3, min = 0.1, max = 60, unit = "s", doc = "Time for one full up-and-down" },
}

function Bobber.on_start(self)
    self.origin = self.entity:position()
    -- Copies in different places bob out of step.
    self.phase = self.origin.x * 0.7 + self.origin.z * 1.3
end

function Bobber.on_update(self, dt: number)
    if not self.props.enabled then
        return
    end
    local w = 2 * math.pi / math.max(self.props.period, 0.1)
    local lift = math.sin(time.elapsed() * w + self.phase) * self.props.height
    self.entity:set_position(self.origin + vector.create(0, lift, 0))
end

return Bobber
