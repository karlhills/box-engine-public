-- The rules: keeps the score from `collected` signals, plays the pickup
-- sound, and when every gem is gone waits a moment and spawns a new ring
-- of them (a batch: one script spawning and counting many entities).
-- Saved games keep the score and the round (on_save / on_load).

local Game = {}

Game.properties = {
    ring = { type = "integer", default = 6, min = 1, max = 64, doc = "Gems per new ring" },
    radius = { type = "number", default = 5, min = 1, max = 30, unit = "m" },
    pickup = { type = "asset", default = "audio/pickup.wav", doc = "Played where a gem is collected" },
}

function Game.on_start(self)
    self.score = 0
    self.round = 1
    events.on("collected", function(ev)
        self.score += ev.points
        log.info("collected +" .. ev.points .. ": score " .. self.score)
        if self.props.pickup then
            events.send("PlaySound", { clip = self.props.pickup, position = ev.at, volume = 0.8 })
        end
    end)
    -- Rounds run as a task: wait until the gems are gone, then refill. (A
    -- task, not a loop in on_start: on_load runs when on_start returns.)
    task.spawn(function()
        while true do
            wait_until(function()
                return #world.scripts("scripts/gem.luau") == 0
            end)
            log.info("round " .. self.round .. " complete")
            wait(1.0)
            self.round += 1
            self:spawn_ring()
        end
    end)
end

function Game.spawn_ring(self)
    local n = self.props.ring
    for i = 1, n do
        local a = (i - 1) / n * math.pi * 2
        world.spawn({
            name = "Gem " .. self.round .. "." .. i,
            Transform = { translation = vector.create(math.cos(a) * self.props.radius, 0.8, math.sin(a) * self.props.radius) },
            Primitive = { shape = { Sphere = { radius = 0.3 } } },
            StandardMaterial = { base_color = Color.hex("#3fd0ff"), emissive = Color.rgb(0.2, 1.2, 1.6), roughness = 0.3 },
            Collider = { shape = { Sphere = { radius = 0.4 } }, is_trigger = true },
            Script = { source = "scripts/gem.luau", props = { points = 10 * self.round } },
        })
    end
end

function Game.on_save(self)
    return { score = self.score, round = self.round }
end

function Game.on_load(self, data)
    self.score = data.score
    self.round = data.round
end

return Game
