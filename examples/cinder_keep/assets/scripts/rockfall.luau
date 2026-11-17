-- A trigger on the upper ledge: when the hero first crosses it, the
-- boulders resting on the rock slope (kinematic until then) become dynamic
-- rigid bodies and get a shove downhill, rolling across the path.

local util = require("./util")

local Rockfall = {}

Rockfall.properties = {
    boulders = { type = "string", default = "Boulder 1,Boulder 2,Boulder 3", doc = "Names of the boulders it releases, comma separated" },
    shove = { type = "vector", default = vector.create(-3, 0, 0), unit = "m/s", doc = "Velocity change given to each boulder" },
}

function Rockfall.on_start(self)
    self.fired = false
end

function Rockfall.on_trigger_enter(self, other: Entity)
    if self.fired or not util.is_player(other) then
        return
    end
    self.fired = true
    log.info("rockfall!")
    for _, name in util.list(self.props.boulders) do
        local boulder = world.find(name)
        if boulder ~= nil then
            boulder:set("RigidBody", { kind = "Dynamic" })
            local mass = boulder:get("RigidBody").mass
            -- The body becomes dynamic at the next fixed step; push it then.
            task.delay(0.05, function()
                if boulder:is_alive() then
                    physics.apply_impulse(boulder, self.props.shove * mass)
                end
            end)
        end
    end
end

function Rockfall.on_save(self)
    return { fired = self.fired }
end

function Rockfall.on_load(self, saved)
    self.fired = saved.fired
end

return Rockfall
