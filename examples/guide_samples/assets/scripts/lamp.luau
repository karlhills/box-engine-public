-- Entity methods and component access (reference: Entity).
local Lamp = {}

function Lamp.on_start(self)
    local e = self.entity
    e:set_position(e:position() + vector.create(0, 1, 0))
    local t = e:get("Transform")               -- a copy
    e:set("Transform", { scale = t.scale * 2 })
    e:set("PointLight.intensity", 40)          -- one field
    if not e:has("Collider") then
        e:add("Collider", { is_trigger = true })
    end
end

return Lamp
