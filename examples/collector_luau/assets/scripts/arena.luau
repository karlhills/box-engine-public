-- An arena: makes sure the game (score, HUD, menus) is running, and when
-- its last gem is collected switches to the next arena.

local util = require("./util")

local Arena = {}

Arena.properties = {
    next_scene = { type = "string", default = "", doc = "The scene to switch to when every gem is collected (a name or a path)" },
}

function Arena.on_start(self)
    -- Runtime entities go with the scene unless marked persistent (the game).
    scenes.set_runtime_entities("despawn_unless_persistent")
    if world.find("Game") == nil then
        world.spawn({ name = "Game", Script = "scripts/game.luau" })
    end
    events.on("collected", function(_)
        if util.gems_left() > 0 then
            return
        end
        events.send("arena_complete", { next_scene = self.props.next_scene })
        if self.props.next_scene ~= "" then
            log.info("arena complete; switching to " .. self.props.next_scene)
            scenes.switch(self.props.next_scene)
        else
            log.info("arena complete; no next scene set")
        end
    end)
end

return Arena
