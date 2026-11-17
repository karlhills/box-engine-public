-- Keeps the score from coin signals and shows it in the HUD that hud.luau
-- builds (scripting guide, reference: events).
local Score = {}

function Score.on_start(self)
    self.score = 0
    events.on("coin_collected", function(payload)
        self.score += payload.value
    end)
end

function Score.on_update(self, dt)
    -- A setter changes nothing (and costs little) when the value is the same.
    if ui.contains("score") then
        ui.set_arg("score", "score", self.score)
    end
end

return Score
