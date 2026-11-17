-- A coin: a trigger collider that sends a signal and a sound when the
-- player touches it (scripting guide, reference: events).
local Coin = {}

function Coin.on_update(self, dt)
    self.entity:rotate_y(3 * dt)
end

function Coin.on_trigger_enter(self, other)
    events.send("coin_collected", { value = 10 })
    events.send("PlaySound", { clip = "audio/pickup.wav", position = self.entity:world_position() })
    self.entity:despawn()
end

return Coin
