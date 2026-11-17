-- The game: score, the HUD, the pause menu and saved games. One runtime
-- entity, spawned by the first arena and kept across scene switches
-- (scenes.persist). Text comes from the string tables in assets/strings
-- (English and German); the menu works with keyboard, mouse and gamepad
-- (d-pad or left stick to move, South to confirm, East or Escape to close,
-- Start/Escape/P to open and close). Saving stores every saved entity plus
-- each script's on_save data; loading restores both.

local util = require("./util")

local Game = {}

Game.properties = {
    slot = { type = "string", default = "quick", doc = "The save slot the pause menu uses" },
}

local HUD = {
    kind = "column", id = "hud", class = "hud", anchor = { 0, 0 }, offset = { 32, 32 },
    children = {
        { kind = "text", id = "score", class = "score", key = "hud.score", args = { score = 0 } },
        { kind = "text", id = "gems", key = "hud.gems", args = { left = 0 } },
    },
}

local MENU = {
    kind = "panel", id = "pause_menu", modal = true, hidden = true, anchor = { 0.5, 0.5 },
    children = {
        { kind = "text", class = "title", key = "menu.paused" },
        { kind = "button", id = "resume", key = "menu.resume", autofocus = true },
        { kind = "button", id = "save", key = "menu.save" },
        { kind = "button", id = "load", key = "menu.load" },
        { kind = "button", id = "language", key = "menu.language" },
    },
}

local TOAST = { kind = "text", id = "toast", class = "toast", hidden = true, anchor = { 1, 0 }, offset = { -32, 32 }, text = "" }

function Game.on_start(self)
    scenes.persist(self.entity)
    self.score = 0
    self.collected = 0
    self.arenas = 0
    self.paused = false
    self.closed_at = -1
    self.toasts = 0
    ui.set_theme("ui/collector.theme.json")
    -- The pause action still reaches the game while the menu is open.
    ui.set_passthrough_actions({ "pause" })
    self.hud = ui.add(HUD)
    self.menu = ui.add(MENU)
    self.toast = ui.add(TOAST)

    events.on("collected", function(ev)
        self.score += ev.points
        self.collected += 1
        log.info("collected +" .. ev.points .. ": score " .. self.score)
        self:refresh()
    end)
    events.on("arena_complete", function(_)
        self.arenas += 1
        log.info("arena complete (" .. self.arenas .. " so far, score " .. self.score .. ")")
    end)
    ui.on("click", "resume", function(_)
        self:close_menu()
    end)
    ui.on("click", "save", function(_)
        save.save(self.props.slot)
        self:close_menu()
    end)
    ui.on("click", "load", function(_)
        save.load(self.props.slot)
        self:close_menu()
    end)
    ui.on("click", "language", function(_)
        strings.set_language(if strings.language() == "de" then "en" else "de")
    end)
    ui.on("back", function(_)
        self:close_menu()
    end)
    save.on(function(result)
        self:show_toast("toast." .. result.kind)
    end)
    self:refresh()
end

function Game.on_update(self, dt: number)
    if input.pressed("pause") then
        if self.paused then
            self:close_menu()
        elseif self.closed_at ~= time.frame() then
            -- (not when Escape closed the menu this very frame)
            self:open_menu()
        end
    end
    local left = util.gems_left()
    if left ~= self.left then
        self.left = left
        ui.set_arg("gems", "left", left)
    end
end

function Game.refresh(self)
    ui.set_arg("score", "score", self.score)
end

function Game.open_menu(self)
    self.paused = true
    time.set_paused(true)
    input.release_cursor()
    ui.set_visible(self.menu, true)
    -- Always open on Resume (menus otherwise reopen where they were left).
    ui.focus("resume")
end

function Game.close_menu(self)
    if not self.paused then
        return
    end
    self.paused = false
    self.closed_at = time.frame()
    time.set_paused(false)
    ui.set_visible(self.menu, false)
end

function Game.show_toast(self, key: string)
    ui.set_key(self.toast, key)
    ui.set_visible(self.toast, true)
    self.toasts += 1
    local mine = self.toasts
    task.delay(2, function()
        if self.toasts == mine then
            ui.set_visible(self.toast, false)
        end
    end)
end

function Game.on_destroy(self)
    -- Loading a save replaces this entity: the new one builds its own UI.
    ui.remove(self.hud)
    ui.remove(self.menu)
    ui.remove(self.toast)
    if self.paused then
        time.set_paused(false)
    end
end

function Game.on_save(self)
    return { score = self.score, collected = self.collected, arenas = self.arenas, language = strings.language() }
end

function Game.on_load(self, saved)
    self.score = saved.score
    self.collected = saved.collected
    self.arenas = saved.arenas
    strings.set_language(saved.language)
    self:refresh()
end

return Game
