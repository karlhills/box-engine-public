-- Drives a glTF character's animation state machine from its movement
-- (reference: animation). `body` is the PrefabInstance of the model.
local Hero = {}

Hero.properties = {
    body = { type = "entity", doc = "The character model (with an AnimationStateMachine)" },
}

function Hero.on_update(self, dt)
    local body = self.props.body
    local v = character.velocity(self.entity)
    animation.set_float(body, "speed", vector.magnitude(vector.create(v.x, 0, v.z)))
    animation.set_bool(body, "grounded", character.is_grounded(self.entity))
    if input.pressed("jump") then
        animation.trigger(body, "jump")
    end
    if animation.state(body) == "Jump" and animation.is_finished(body) then
        log.info("jump clip done")
    end
end

return Hero
