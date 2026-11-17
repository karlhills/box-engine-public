-- String tables (reference: strings).
local Lang = {}

function Lang.on_start(self)
    log.info(strings.format("hud.score", { score = 42 }))   -- "Score 42"
    strings.set_language("de")
end

return Lang
