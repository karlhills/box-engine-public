-- Counts collected coins and saves them with F5 / F9 (reference: save).
local Game = {}

function Game.on_start(self)
    self.coins = 0                 -- defaults first: on_load runs after this
    events.on("coin_collected", function()
        self.coins += 1
    end)
    save.on(function(result)
        log.info(result.kind, result.slot, result.message or "")
    end)
end

function Game.on_update(self, dt)
    if input.key_pressed("F5") then save.save("quick") end
    if input.key_pressed("F9") then save.load("quick") end
end

function Game.on_save(self)
    return { coins = self.coins }
end

function Game.on_load(self, data)
    self.coins = data.coins
end

return Game
