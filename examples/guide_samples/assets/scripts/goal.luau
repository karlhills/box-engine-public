-- Switches to the second level when the player reaches it (reference: scenes).
local Goal = {}

function Goal.on_trigger_enter(self, other)
    if other:name() == "Player" then
        scenes.switch("level2")
    end
end

return Goal
