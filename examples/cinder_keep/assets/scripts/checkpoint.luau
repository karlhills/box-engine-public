-- A checkpoint brazier: the first time the hero touches its trigger it
-- lights (its flame emitter starts, its light brightens) and the game saves
-- the checkpoint slot. The lit state is scene data (the emitter and light
-- components), so a loaded checkpoint shows it lit.

local util = require("./util")

local Checkpoint = {}

Checkpoint.properties = {
    id = { type = "string", default = "", doc = "Name for logs" },
    flame = { type = "entity", doc = "Particle emitter started when lit" },
    lit_intensity = { type = "number", default = 25, min = 0, max = 500, doc = "Point light intensity once lit" },
}

function Checkpoint.on_start(self)
    self.reached = false
end

function Checkpoint.on_trigger_enter(self, other: Entity)
    if self.reached or not util.is_player(other) then
        return
    end
    self.reached = true
    if self.props.flame ~= nil then
        particles.start(self.props.flame)
    end
    if self.entity:has("PointLight") then
        self.entity:set("PointLight", { intensity = self.props.lit_intensity })
    end
    particles.burst(self.entity:world_position() + vector.create(0, 1.3, 0), 40, {
        speed = vector.create(1, 4, 0),
        lifetime = vector.create(0.5, 1.0),
        gravity = vector.create(0, 2, 0),
        color = {
            { t = 0, color = Color.rgb(1.0, 0.8, 0.3, 1) },
            { t = 1, color = Color.rgb(1.0, 0.3, 0.05, 0) },
        },
        intensity = 6,
        blend = "Additive",
    })
    events.send("checkpoint", { id = self.props.id })
end

function Checkpoint.on_save(self)
    return { reached = self.reached }
end

function Checkpoint.on_load(self, saved)
    self.reached = saved.reached
end

return Checkpoint
