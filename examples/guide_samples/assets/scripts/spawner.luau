-- Reads a data table once it has been imported (reference: data).
local Spawner = {}

function Spawner.on_start(self)
    local enemies = nil
    wait_until(function()
        enemies = data.table("data/enemies.data.json")
        return enemies ~= nil
    end)
    log.info("slime hp", enemies.slime.hp)
end

return Spawner
