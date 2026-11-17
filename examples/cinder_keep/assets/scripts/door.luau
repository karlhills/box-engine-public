-- A doorway trigger: the hero walking in asks the game to move to `scene`
-- (it fades out, switches scenes and fades in; health and score carry).
-- `victory` instead ends the game (the throne room).

local util = require("./util")

local Door = {}

Door.properties = {
    scene = { type = "string", default = "", doc = "The scene to enter (a name or a path)" },
    victory = { type = "boolean", default = false, doc = "Win the game instead of changing scenes" },
}

function Door.on_trigger_enter(self, other: Entity)
    if not util.is_player(other) then
        return
    end
    if self.props.victory then
        events.send("victory", {})
    elseif self.props.scene ~= "" then
        events.send("door", { scene = self.props.scene })
    end
end

return Door
