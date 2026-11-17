-- A ray down and an impulse for the crates around (reference: physics).
local Lift = {}

function Lift.on_start(self)
    wait(0.5)   -- queries see colliders from the next frame on
    local here = self.entity:world_position()
    local hit = physics.raycast(here, vector.create(0, -1, 0), 10, { exclude = self.entity })
    if hit then
        log.info("above", hit.entity:name(), "at", hit.distance, "m")
    end
    for _, e in physics.overlap_sphere(here, 3) do
        if e:has("RigidBody") then
            physics.apply_impulse(e, vector.create(0, 50, 0))   -- N·s: 5 m/s for 10 kg
        end
    end
end

return Lift
