-- Lamps on the ground at the intent's points (docs/reference/ground.md).
local Lamps = { scope = "intent" }

function Lamps.rule(intents, out)
    for _, it in intents do
        for i, p in it.points do
            local y = ground.height(p.x, p.z) or p.y
            out:part(it, `lamp{i}`, { Primitive = { shape = "Cylinder" } }, { position = vector.create(p.x, y + 1, p.z) })
        end
    end
end

return Lamps
