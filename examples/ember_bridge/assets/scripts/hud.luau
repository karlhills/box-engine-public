-- The HUD: how many embers the player carries. The puzzle itself needs no
-- script: the embers and braziers are `Interactable` components (see
-- authoring/main.commands.json). They change the `embers` game variable,
-- and this script only shows it.
--
-- The game UI is retained: build the nodes once with `ui.add`, then change
-- them with setters when something changes, not every frame.

local Hud = {}

local function embers_text(n: number): string
    return if n == 1 then "1 ember" else `{n} embers`
end

function Hud.on_start(self)
    self.panel = ui.add({
        kind = "panel", anchor = { 0, 0 }, offset = { 24, 24 },
        style = { background = "#00000080", radius = 10, padding = { 10, 18 }, gap = 4 },
        children = {
            { kind = "text", id = "embers", text = embers_text(vars.get("embers")), style = { text_size = 28 } },
            { kind = "text", id = "goal", text = "Light both braziers to cross the water", style = { text_size = 18, text_color = "#d8cfc0" } },
        },
    })
    -- Called after an Interactable (or a loaded save) changes the variable.
    vars.on_change("embers", function(change)
        ui.set_text("embers", embers_text(change.value))
    end)
    -- Every use of an interactable sends `Interacted`.
    events.on("Interacted", function(e)
        local used = world.entity(e.entity)
        if used ~= nil and string.find(used:name(), "Brazier") then
            ui.set_text("goal", "A bridge rises from the water")
        end
    end)
end

-- Remove the UI when the entity goes away (a scene switch, a loaded save).
function Hud.on_destroy(self)
    ui.remove(self.panel)
end

return Hud
