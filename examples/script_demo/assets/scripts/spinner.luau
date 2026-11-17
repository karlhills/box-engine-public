-- Spins its entity around an axis and bobs it up and down.

local util = require("./util")

local Spinner = {}

Spinner.properties = {
    speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s", doc = "Turn rate" },
    axis = { default = vector.create(0, 1, 0), doc = "Turn axis" },
    bob = { type = "number", default = 0.15, min = 0, max = 2, unit = "m", doc = "Bob height" },
}

function Spinner.on_start(self)
    self.home = self.entity:position()
    self.t = 0
end

function Spinner.on_update(self, dt: number)
    self.t += dt
    local turn = Quat.from_axis_angle(self.props.axis, math.rad(self.props.speed) * dt)
    self.entity:rotate(turn)
    local lift = util.bob(self.t, self.home) * self.props.bob
    self.entity:set_position(self.home + vector.create(0, lift, 0))
end

return Spinner
