-- A colonnade along the intent's points: turned columns (the built-in
-- Lathe generator) at most `spacing` apart, and a beam on top of each
-- stretch between two points.

local Colonnade = {}

Colonnade.properties = {
    spacing = { type = "number", default = 2.5, min = 1, max = 10, unit = "m", doc = "Largest distance between two columns" },
    height = { type = "number", default = 3.0, min = 1, max = 12, unit = "m", doc = "Column height" },
    radius = { type = "number", default = 0.22, min = 0.05, max = 1, unit = "m", doc = "Shaft radius at the foot" },
    beams = { type = "boolean", default = true, doc = "A beam on top of the columns" },
    stone = { type = "color", default = Color.srgb(0.86, 0.82, 0.74), doc = "Stone colour" },
}

function Colonnade.rule(intents: { RuleIntent }, out: RuleOut)
    for _, it in intents do
        local p = it.props
        local pts = it.points
        if #pts < 2 then
            out:warn(it, "a colonnade needs at least two points")
            continue
        end
        if p.spacing < p.radius * 3 then
            out:warn(it, "the columns are closer than one and a half shaft widths")
        end
        local stone = { base_color = p.stone, roughness = 0.7 }
        local column = {
            Lathe = { shape = { Column = { height = p.height, radius = p.radius } }, segments = 24 },
            StandardMaterial = stone,
        }
        for i = 1, #pts - 1 do
            local a, b = pts[i], pts[i + 1]
            local d = b - a
            local length = vector.magnitude(d)
            local bays = math.max(1, math.ceil(length / p.spacing - 1e-4))
            -- Columns: the first point of every stretch, and the last point.
            for k = 0, bays - 1 do
                out:part(it, `bay{i}/column{k}`, column, { position = a + d * (k / bays) })
            end
            if p.beams and length > 0 then
                local yaw = math.atan2(-d.z, d.x)
                local depth = p.radius * 2.6
                out:part(it, `bay{i}/beam`, {
                    Primitive = { shape = { Cuboid = { size = vector.create(length + depth, depth * 0.6, depth) } } },
                    StandardMaterial = stone,
                }, {
                    position = a + d * 0.5 + vector.create(0, p.height + depth * 0.3, 0),
                    rotation = Quat.from_rotation_y(yaw),
                })
            end
        end
        out:part(it, "end/column", column, { position = pts[#pts] })
    end
end

return Colonnade
