-- Spinner, as a script: turns the entity around an axis, forever (coins,
-- fans, windmills). Made by Convert to Script from the Spinner behaviour;
-- each entity keeps its own values in the Script's properties. Change
-- anything: this file is yours now.

local Spinner = {}

Spinner.properties = {
    enabled = { type = "boolean", default = true, doc = "Off: it stops where it is" },
    axis = { type = "vector", default = vector.create(0, 1, 0), doc = "The axis it turns around, in its parent's axes (Y: like a coin)" },
    speed = { type = "number", default = 2, min = -50, max = 50, unit = "rad/s", doc = "Turn rate; negative turns the other way" },
}

function Spinner.on_start(self)
    -- Turns are measured from the rotation it starts with.
    self.base = self.entity:rotation()
    self.angle = 0
end

function Spinner.on_update(self, dt: number)
    if not self.props.enabled then
        return
    end
    self.angle = (self.angle + self.props.speed * dt) % (2 * math.pi)
    local axis = self.props.axis
    if vector.magnitude(axis) < 1e-6 then
        axis = vector.create(0, 1, 0)
    end
    self.entity:set_rotation(Quat.from_axis_angle(axis, self.angle) * self.base)
end

return Spinner
