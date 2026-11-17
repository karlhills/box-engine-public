-- Moves a KinematicCharacter with the "move" and "jump" actions
-- (scripting guide, reference: character).
local Player = {}

Player.properties = {
    speed = { type = "number", default = 5, min = 0, max = 20, unit = "m/s", doc = "Walking speed" },
}

function Player.on_fixed_update(self, dt)
    local move = input.axis2("move")
    character.move(self.entity, vector.create(move.x, 0, -move.y) * self.props.speed)
    if input.pressed("jump") and character.jump(self.entity) then
        events.send("jumped")
    end
end

return Player
