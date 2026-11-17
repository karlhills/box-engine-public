-- Spawning, queries and resources (reference: world).
local Level = {}

function Level.on_start(self)
    world.spawn({
        name = "Marker",
        Transform = { translation = vector.create(0, 3, 0) },
        Primitive = { shape = { Sphere = { radius = 0.3 } } },
    })
    for _, e in world.query("Collider") do
        log.info(e:name(), e:world_position())
    end
    local env = world.resource("Environment")
    world.set_resource("Environment", { exposure = env.exposure * 1.2 })
end

return Level
