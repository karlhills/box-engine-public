-- A light that pulses on a timer: a coroutine-style loop with waits,
-- no hand-written state machine.

local Beacon = {}

Beacon.properties = {
    period = { type = "number", default = 0.6, min = 0.05, max = 5, unit = "s", doc = "Time on, then off" },
    bright = { type = "number", default = 40, min = 0, max = 1000, unit = "cd" },
}

function Beacon.on_start(self)
    while true do
        self.entity:set("PointLight", { intensity = self.props.bright })
        wait(self.props.period)
        self.entity:set("PointLight", { intensity = self.props.bright * 0.1 })
        wait(self.props.period)
    end
end

return Beacon
