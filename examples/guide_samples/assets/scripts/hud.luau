-- A HUD and a hidden pause menu (reference: ui).
local Hud = {}

function Hud.on_start(self)
    ui.add({
        kind = "panel", id = "hud", anchor = { 0, 0 }, offset = { 24, 24 },
        children = {
            { kind = "text", id = "score", key = "hud.score", args = { score = 0 } },
        },
    })
    ui.set_arg("score", "score", 10)

    local menu = ui.add({
        kind = "panel", id = "pause", modal = true, hidden = true, anchor = { 0.5, 0.5 },
        children = {
            { kind = "button", id = "resume", text = "Resume" },
            { kind = "button", id = "quit", text = "Quit" },
        },
    })
    ui.on("click", "resume", function()
        ui.set_visible(menu, false)
        time.set_paused(false)
    end)
end

return Hud
