-- The old warden at the start: turns to watch the hero, and the first time
-- the hero comes within its trigger it speaks (the game shows the lines,
-- string keys from the string tables, one per confirm).

local util = require("./util")

local Warden = {}

Warden.properties = {
    speaker = { type = "string", default = "dialogue.warden.name", doc = "String key of the speaker's name" },
    lines = { type = "string", default = "dialogue.warden.1,dialogue.warden.2,dialogue.warden.3,dialogue.warden.4", doc = "String keys of the lines, comma separated" },
    watch_distance = { type = "number", default = 8, min = 0, max = 50, unit = "m", doc = "Turns toward the hero within this distance" },
}

function Warden.on_start(self)
    self.talked = false
    self.yaw = util.yaw_of(self.entity:forward())
end

function Warden.on_update(self, dt: number)
    local hero = world.find("Player")
    if hero == nil then
        return
    end
    local to = hero:position() - self.entity:position()
    to = vector.create(to.x, 0, to.z)
    if vector.magnitude(to) < self.props.watch_distance then
        self.yaw = util.turn_towards(self.yaw, util.yaw_of(to), 3 * dt)
        self.entity:set_rotation(Quat.from_rotation_y(self.yaw))
    end
end

function Warden.on_trigger_enter(self, other: Entity)
    if self.talked or not util.is_player(other) then
        return
    end
    self.talked = true
    events.send("dialogue", { id = "warden", speaker = self.props.speaker, lines = util.list(self.props.lines) })
end

function Warden.on_save(self)
    return { talked = self.talked }
end

function Warden.on_load(self, saved)
    self.talked = saved.talked
end

return Warden
