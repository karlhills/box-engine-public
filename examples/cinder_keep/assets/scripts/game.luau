-- The game: health, score, the HUD, the pause menu, dialogue, checkpoints,
-- death, scene transitions and the win screen. One runtime entity, spawned
-- by the first level (level.luau) and kept across scene switches
-- (scenes.persist), so score and health carry from the fields into the
-- keep. Text comes from the string tables (English and German).
--
-- Signals it handles: `damage` {amount}, `player_fell`, `enemy_killed`
-- {points}, `wave_started` {count}, `wave_progress` {left},
-- `wave_cleared`, `checkpoint` {id}, `area_entered` {title, checkpoint},
-- `dialogue` {id, speaker, lines}, `door` {scene}, `victory`. It sends
-- `player_hit` {amount} and `player_died`.
--
-- Checkpoints are the save slot `checkpoint`: touching a checkpoint and
-- entering an area save it; dying and "Restart from checkpoint" load it
-- (every saved entity plus each script's on_save data). Loading replaces
-- this entity; the new one fades the screen back in.

local util = require("./util")

local Game = {}

Game.properties = {
    slot = { type = "string", default = "checkpoint", doc = "The save slot checkpoints use" },
    first_scene = { type = "string", default = "outdoor", doc = "Where Play again starts" },
}

local HUD = {
    kind = "column", id = "hud", class = "hud", anchor = { 0, 0 }, offset = { 32, 32 },
    children = {
        { kind = "row", children = {
            { kind = "progress", id = "health_bar", class = "health_bar", value = 1, min = 0, max = 1 },
            { kind = "text", id = "health", key = "hud.health", args = { hp = 100 } },
        } },
        { kind = "text", id = "score", class = "score", key = "hud.score", args = { score = 0 } },
        { kind = "text", id = "enemies", key = "hud.enemies", args = { left = 0 }, hidden = true },
    },
}

local AREA = { kind = "text", id = "area", class = "area", anchor = { 0.5, 0.2 }, hidden = true, text = "" }

local TOAST = { kind = "text", id = "toast", class = "toast", hidden = true, anchor = { 1, 0 }, offset = { -32, 32 }, text = "" }

local MENU = {
    kind = "panel", id = "pause_menu", modal = true, hidden = true, anchor = { 0.5, 0.5 },
    children = {
        { kind = "text", class = "title", key = "menu.paused" },
        { kind = "button", id = "resume", key = "menu.resume", autofocus = true },
        { kind = "button", id = "restart", key = "menu.restart" },
        { kind = "button", id = "language", key = "menu.language" },
        { kind = "button", id = "quit", key = "menu.quit" },
    },
}

local DIALOGUE = {
    kind = "panel", id = "dialogue", class = "dialogue", modal = true, hidden = true, anchor = { 0.5, 0.94 },
    children = {
        { kind = "text", id = "dialogue_speaker", class = "speaker", text = "" },
        { kind = "text", id = "dialogue_line", class = "line", text = "" },
        { kind = "button", id = "dialogue_next", key = "dialogue.continue", autofocus = true },
    },
}

local WIN = {
    kind = "panel", id = "win", modal = true, hidden = true, anchor = { 0.5, 0.5 },
    children = {
        { kind = "text", class = "title", key = "win.title" },
        { kind = "text", id = "win_score", key = "win.score", args = { score = 0 } },
        { kind = "button", id = "again", key = "win.again", autofocus = true },
        { kind = "button", id = "win_quit", key = "win.quit" },
    },
}

local FADE = { kind = "stack", id = "fade", class = "fade", layer = 500, anchor = { 0, 0 } }

function Game.on_start(self)
    scenes.persist(self.entity)
    -- Defaults: a loaded checkpoint's on_load runs after on_start returns
    -- and overrides them.
    self.max_health = 100
    self.score = 0
    self.kills = 0
    self.hurt_at = -100
    self.paused = false
    self.closed_at = -1
    self.toasts = 0
    self.areas = 0
    self.busy = nil -- "dying", "moving", "restarting" while a fade runs
    self.talking = false
    self.won = false
    self.fade_alpha = 1
    self.fade_job = 0
    ui.set_theme("ui/cinder.theme.json")
    -- The pause action still reaches the game while the menu is open.
    ui.set_passthrough_actions({ "pause" })
    self.ui = {
        hud = ui.add(HUD),
        area = ui.add(AREA),
        toast = ui.add(TOAST),
        menu = ui.add(MENU),
        dialogue = ui.add(DIALOGUE),
        win = ui.add(WIN),
        fade = ui.add(FADE),
    }
    self:set_fade(1)

    events.on("damage", function(ev)
        self:hurt(ev.amount)
    end)
    events.on("player_fell", function(_)
        self.health = 0
        self:refresh()
        self:die("fell")
    end)
    events.on("enemy_killed", function(ev)
        self.score += ev.points
        self.kills += 1
        self:refresh()
    end)
    events.on("wave_started", function(ev)
        ui.set_arg("enemies", "left", ev.count)
        ui.set_visible("enemies", true)
        self:show_toast("toast.wave")
    end)
    events.on("wave_progress", function(ev)
        ui.set_arg("enemies", "left", ev.left)
    end)
    events.on("wave_cleared", function(_)
        ui.set_visible("enemies", false)
        self:show_toast("toast.gate_open")
    end)
    events.on("checkpoint", function(ev)
        log.info("checkpoint " .. ev.id)
        save.save(self.props.slot)
        self:show_toast("toast.checkpoint")
    end)
    events.on("area_entered", function(ev)
        self.areas += 1
        if ev.title ~= "" then
            self:show_area(ev.title)
        end
        if ev.checkpoint then
            save.save(self.props.slot)
        end
    end)
    events.on("dialogue", function(ev)
        self:run_dialogue(ev)
    end)
    events.on("door", function(ev)
        self:go_to(ev.scene)
    end)
    events.on("victory", function(_)
        self:win()
    end)

    ui.on("click", "resume", function(_)
        self:close_menu()
    end)
    ui.on("click", "restart", function(_)
        self:close_menu()
        self:reload("restarting")
    end)
    ui.on("click", "language", function(_)
        strings.set_language(if strings.language() == "de" then "en" else "de")
    end)
    ui.on("click", "quit", function(_)
        app.quit()
    end)
    ui.on("back", function(_)
        self:close_menu()
    end)
    ui.on("click", "dialogue_next", function(_)
        self.advanced = true
    end)
    ui.on("click", "again", function(_)
        self:play_again()
    end)
    ui.on("click", "win_quit", function(_)
        app.quit()
    end)
    save.on(function(result)
        if result.kind == "failed" then
            log.warn("save " .. result.slot .. " failed: " .. (result.message or "?"))
            self:show_toast("toast.failed")
        end
    end)
    self.tuning = util.wait_for_tuning()
    self.max_health = self.tuning.player.max_health
    self.health = self.max_health
    self:refresh()
    -- Fade in (at the start and after loading a checkpoint).
    self:fade_to(0, 0.8)
end

function Game.on_update(self, dt: number)
    if input.pressed("pause") then
        if self.paused then
            self:close_menu()
        elseif self.closed_at ~= time.frame() and self.busy == nil and not self.talking and not self.won then
            -- (not when Escape closed the menu this very frame)
            self:open_menu()
        end
    end
end

function Game.refresh(self)
    if self.health == nil then
        return
    end
    local hp = math.max(0, math.ceil(self.health))
    ui.set_arg("health", "hp", hp)
    ui.set_value("health_bar", self.health / self.max_health)
    ui.set_arg("score", "score", self.score)
end

-- ---- health and death ----

function Game.hurt(self, amount: number)
    if self.tuning == nil or self.busy ~= nil or self.won or self.health <= 0 then
        return
    end
    local now = time.elapsed()
    if now - self.hurt_at < self.tuning.player.invulnerable then
        return
    end
    self.hurt_at = now
    self.health = math.max(0, self.health - amount)
    self:refresh()
    events.send("player_hit", { amount = amount })
    if self.health <= 0 then
        self:die("hurt")
    end
end

function Game.die(self, cause: string)
    if self.busy ~= nil then
        return
    end
    log.info("the hero died (" .. cause .. "); back to the checkpoint")
    events.send("player_died", { cause = cause })
    self:reload("dying")
end

-- Fades out and loads the checkpoint (the loaded game fades back in).
function Game.reload(self, why: string)
    if self.busy ~= nil then
        return
    end
    self.busy = why
    task.spawn(function()
        if why == "dying" then
            wait(0.5)
        end
        self:fade_to(1, 0.5)
        wait_until(function()
            return self.fade_alpha >= 1
        end)
        save.load(self.props.slot)
    end)
end

-- ---- scenes ----

function Game.go_to(self, scene: string)
    if self.busy ~= nil then
        return
    end
    self.busy = "moving"
    task.spawn(function()
        self:fade_to(1, 0.5)
        wait_until(function()
            return self.fade_alpha >= 1
        end)
        log.info("entering " .. scene)
        scenes.switch(scene)
        wait_frames(3)
        self.busy = nil
        self:fade_to(0, 0.6)
    end)
end

function Game.show_area(self, key: string)
    ui.set_key(self.ui.area, key)
    ui.set_visible(self.ui.area, true)
    local mine = self.areas
    task.delay(2.5, function()
        if self.areas == mine then
            ui.set_visible(self.ui.area, false)
        end
    end)
end

-- ---- the fade overlay ----

function Game.set_fade(self, alpha: number)
    self.fade_alpha = alpha
    ui.set_style(self.ui.fade, { opacity = alpha })
    ui.set_visible(self.ui.fade, alpha > 0.001)
end

-- Fades to `target` over `seconds` (game time) in a task; a newer fade
-- replaces a running one.
function Game.fade_to(self, target: number, seconds: number)
    self.fade_job += 1
    local job = self.fade_job
    task.spawn(function()
        local from = self.fade_alpha
        local t = 0
        while t < seconds and self.fade_job == job do
            wait_frames(1)
            t += time.delta()
            self:set_fade(from + (target - from) * math.min(t / seconds, 1))
        end
        if self.fade_job == job then
            self:set_fade(target)
        end
    end)
end

-- ---- dialogue ----

-- Shows each line of `ev.lines` (string keys) until confirmed (South,
-- Enter, Space or a click on Continue). The panel is modal, so gameplay
-- input is captured while it is open.
function Game.run_dialogue(self, ev)
    if self.talking then
        return
    end
    self.talking = true
    task.spawn(function()
        ui.set_key("dialogue_speaker", ev.speaker)
        ui.set_visible(self.ui.dialogue, true)
        ui.focus("dialogue_next")
        for i, key in ev.lines do
            ui.set_key("dialogue_line", key)
            self.line = i
            self.advanced = false
            wait_until(function()
                return self.advanced
            end)
        end
        ui.set_visible(self.ui.dialogue, false)
        self.talking = false
        self.line = nil
        log.info("dialogue " .. ev.id .. " finished")
        events.send("dialogue_finished", { id = ev.id })
    end)
end

-- ---- menus ----

function Game.open_menu(self)
    self.paused = true
    time.set_paused(true)
    input.release_cursor()
    ui.set_visible(self.ui.menu, true)
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
    ui.set_visible(self.ui.menu, false)
end

function Game.show_toast(self, key: string)
    ui.set_key(self.ui.toast, key)
    ui.set_visible(self.ui.toast, true)
    self.toasts += 1
    local mine = self.toasts
    task.delay(2.5, function()
        if self.toasts == mine then
            ui.set_visible(self.ui.toast, false)
        end
    end)
end

-- ---- winning ----

function Game.win(self)
    if self.won then
        return
    end
    self.won = true
    log.info("victory: score " .. self.score)
    input.release_cursor()
    ui.set_arg("win_score", "score", self.score)
    ui.set_visible(self.ui.win, true)
    ui.focus("again")
end

function Game.play_again(self)
    ui.set_visible(self.ui.win, false)
    self.won = false
    self.health = self.max_health
    self.score = 0
    self.kills = 0
    self:refresh()
    self:go_to(self.props.first_scene)
end

-- ---- saves ----

function Game.on_destroy(self)
    -- Loading a checkpoint replaces this entity: the new one builds its UI.
    for _, node in self.ui or {} do
        ui.remove(node)
    end
    if self.paused then
        time.set_paused(false)
    end
end

function Game.on_save(self)
    return {
        score = self.score,
        health = self.health,
        kills = self.kills,
        language = strings.language(),
    }
end

function Game.on_load(self, saved)
    self.score = saved.score
    self.health = saved.health
    self.kills = saved.kills
    strings.set_language(saved.language)
    self:refresh()
end

return Game
