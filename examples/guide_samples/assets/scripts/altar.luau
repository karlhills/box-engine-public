-- A script on an Interactable (guide: Interactions; reference: interact,
-- vars). The Altar's `Interactable` (a Touch trigger) adds an offering;
-- the script only reacts.
local Altar = {}

function Altar.on_start(self)
    self.offerings = 0
    vars.on_change("offerings", function(change)
        self.offerings = change.value
    end)
    events.on("Interacted", function(e)
        local used = world.entity(e.entity)
        if used ~= nil then
            log.info(used:name() .. " was used")
        end
    end)
end

function Altar.on_interact(self, interactor: Entity)
    self.used_by = interactor:name()
    vars.add("visits", 1)
end

return Altar
