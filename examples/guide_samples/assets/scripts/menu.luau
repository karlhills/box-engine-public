-- Pausing and quitting (reference: time, app). The pause menu itself is
-- built by hud.luau.
local Menu = {}

function Menu.on_start(self)
    ui.on("click", "quit", function() app.quit() end)
end

function Menu.on_update(self, dt)
    if input.pressed("pause") then
        time.set_paused(not time.is_paused())
    end
end

return Menu
