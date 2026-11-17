-- A gem: spins, bobs, and is a trigger the player collects (its collider
-- only reports the player's layer). Collecting plays the pickup sound
-- where it was, bursts into sparkles, sends the `collected` signal (the
-- game keeps the score, the arena counts what is left) and despawns.

local Gem = {}

Gem.properties = {
    points = { type = "integer", default = 10, min = 0, max = 1000, doc = "Score for collecting it" },
    spin_speed = { type = "number", default = 2, min = -20, max = 20, unit = "rad/s" },
    bob_height = { type = "number", default = 0.15, min = 0, max = 2, unit = "m", doc = "How far it bobs up and down" },
    pickup_sound = { type = "asset", default = "audio/pickup.wav", doc = "Played where it is collected" },
}

function Gem.on_start(self)
    self.home = self.entity:position()
    -- Out of step with the other gems.
    self.phase = (self.home.x * 0.7 + self.home.z * 1.3) % (2 * math.pi)
end

function Gem.on_update(self, dt: number)
    self.entity:rotate_y(self.props.spin_speed * dt)
    local lift = math.sin(time.elapsed() * 2 + self.phase) * self.props.bob_height
    self.entity:set_position(self.home + vector.create(0, lift, 0))
end

function Gem.on_trigger_enter(self, other: Entity)
    if other:name() ~= "Player" then
        return
    end
    local at = self.entity:world_position()
    if self.props.pickup_sound then
        events.send("PlaySound", { clip = self.props.pickup_sound, position = at, min_distance = 2, max_distance = 40 })
    end
    local tuning = data.table("data/tuning.data.json")
    local sparkles = if tuning then tuning.pickup.sparkles else 24
    local speed = if tuning then tuning.pickup.sparkle_speed else 3
    particles.burst(at, sparkles, {
        speed = vector.create(speed * 0.5, speed, 0),
        lifetime = vector.create(0.4, 0.8),
        gravity = vector.create(0, -6, 0),
        color = {
            { t = 0, color = Color.rgb(1.0, 0.75, 0.2, 1) },
            { t = 1, color = Color.rgb(1.0, 0.4, 0.05, 0) },
        },
        intensity = 6,
        blend = "Additive",
    })
    events.send("collected", { points = self.props.points, gem = self.entity })
    self.entity:despawn()
end

-- Saved games keep where it bobs around (the saved transform is wherever
-- the bob had it).
function Gem.on_save(self)
    return { home = self.home }
end

function Gem.on_load(self, saved)
    self.home = saved.home
end

return Gem
