-- A level: makes sure the game (game.luau: health, score, HUD, menus) is
-- running, and announces the area once the scene is up: the game shows its
-- title and saves the checkpoint there. Not after a checkpoint was loaded
-- into this scene (the load already put everything back).

local Level = {}

Level.properties = {
    title = { type = "string", default = "", doc = "String key of the area's name, shown on arrival" },
    checkpoint = { type = "boolean", default = true, doc = "Save the checkpoint when the area is entered" },
}

function Level.on_start(self)
    -- Runtime entities go with the scene unless marked persistent (the game).
    scenes.set_runtime_entities("despawn_unless_persistent")
    if world.find("Game") == nil then
        world.spawn({ name = "Game", Script = "scripts/game.luau" })
    end
    self.restored = false
    -- Two frames: the game has started and the scene's scripts are running.
    -- A task, so on_start returns at once and a loaded checkpoint's on_load
    -- (which runs when on_start returns) marks the level restored first.
    task.spawn(function()
        wait_frames(2)
        if not self.restored then
            events.send("area_entered", { title = self.props.title, checkpoint = self.props.checkpoint })
        end
    end)
end

function Level.on_save(self)
    return {}
end

function Level.on_load(self, _)
    self.restored = true
end

return Level
