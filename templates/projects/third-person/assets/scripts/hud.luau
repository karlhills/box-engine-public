-- The HUD: a panel in the top-left corner with the score and how many
-- pickups are left. The score comes from the `picked_up` event that
-- pickup.luau sends; what is left is counted from the pickup scripts.
--
-- The game UI is retained: build the nodes once with `ui.add`, then change
-- them with setters (`ui.set_text`) when something changes, not every frame.

local Hud = {}

function Hud.on_start(self)
    self.score = 0
    self.panel = ui.add({
        kind = "panel", anchor = { 0, 0 }, offset = { 24, 24 },
        style = { background = "#00000080", radius = 10, padding = { 10, 18 }, gap = 4 },
        children = {
            { kind = "text", id = "score", text = "Score 0", style = { text_size = 30 } },
            { kind = "text", id = "left", text = "" },
        },
    })
    events.on("picked_up", function(ev)
        self.score += ev.points
        ui.set_text("score", `Score {self.score}`)
    end)
end

function Hud.on_update(self, dt: number)
    -- Every pickup that is still there (a despawned one is no longer alive).
    local left = 0
    for _, pickup in world.scripts("scripts/pickup.luau") do
        if pickup.entity:is_alive() then
            left += 1
        end
    end
    if left ~= self.left then
        self.left = left
        ui.set_text("left", if left > 0 then `{left} left` else "All collected!")
    end
end

-- Remove the UI when the entity goes away (a scene switch, a loaded save).
function Hud.on_destroy(self)
    ui.remove(self.panel)
end

return Hud
