-- Turns the entity around its Y axis (getting started, script modules).
local Spinner = {}

Spinner.properties = {
    speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s", doc = "Turn rate" },
}

function Spinner.on_update(self, dt)
    self.entity:rotate_y(math.rad(self.props.speed) * dt)
end

return Spinner
