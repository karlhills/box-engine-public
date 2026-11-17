-- A camera that follows a target from behind and above, easing toward it.

local Follow = {}

Follow.properties = {
    target = { type = "entity", doc = "What to follow" },
    offset = { default = vector.create(0, 3.5, 7), doc = "Where the camera sits relative to the target" },
    smoothing = { type = "number", default = 4, min = 0.5, max = 30, unit = "1/s" },
}

function Follow.on_update(self, dt: number)
    local target: Entity? = self.props.target
    if target == nil or not target:is_alive() then
        return
    end
    local focus = target:position()
    local want = focus + self.props.offset
    local ease = 1 - math.exp(-self.props.smoothing * dt)
    local at = self.entity:position()
    self.entity:set_position(at + (want - at) * ease)
    self.entity:look_at(focus + vector.create(0, 0.5, 0))
end

return Follow
