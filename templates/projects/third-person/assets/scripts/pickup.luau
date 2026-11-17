-- A pickup: spins and bobs until the player touches it, then plays a
-- sound, bursts into sparkles, tells the HUD (the `picked_up` event) and
-- disappears.
--
-- The entity's Collider is a trigger on physics layer 4 with mask 2: it
-- reports only things on layer 2, the player's layer, and blocks nothing.
-- Copy a pickup (Cmd+D in the editor) to add more; hud.luau counts them.

local Pickup = {}

Pickup.properties = {
    points = { type = "integer", default = 1, min = 0, max = 100, doc = "What it adds to the score" },
    spin_speed = { type = "number", default = 2, min = -20, max = 20, unit = "rad/s" },
    bob_height = { type = "number", default = 0.15, min = 0, max = 2, unit = "m", doc = "How far it bobs up and down" },
    sound = { type = "asset", default = "audio/pickup.wav", doc = "Played where it is picked up" },
}

function Pickup.on_start(self)
    self.home = self.entity:position()
    -- Each pickup bobs out of step with the others.
    self.phase = (self.home.x * 0.7 + self.home.z * 1.3) % (2 * math.pi)
end

function Pickup.on_update(self, dt: number)
    self.entity:rotate_y(self.props.spin_speed * dt)
    local lift = math.sin(time.elapsed() * 2 + self.phase) * self.props.bob_height
    self.entity:set_position(self.home + vector.create(0, lift, 0))
end

-- Called by physics when something enters the trigger.
function Pickup.on_trigger_enter(self, other: Entity)
    if other:name() ~= "Player" then
        return
    end
    local at = self.entity:world_position()
    if self.props.sound then
        events.send("PlaySound", { clip = self.props.sound, position = at, min_distance = 2, max_distance = 40 })
    end
    particles.burst(at, 24, {
        speed = vector.create(1.5, 3, 0),
        lifetime = vector.create(0.4, 0.8),
        gravity = vector.create(0, -6, 0),
        color = {
            { t = 0, color = Color.rgb(1.0, 0.75, 0.2, 1) },
            { t = 1, color = Color.rgb(1.0, 0.4, 0.05, 0) },
        },
        intensity = 6,
        blend = "Additive",
    })
    events.send("picked_up", { points = self.props.points })
    self.entity:despawn()
end

return Pickup
