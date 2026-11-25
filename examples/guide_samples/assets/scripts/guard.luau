-- A patrolling guard that chases the player (reference: nav).
local Guard = {}

-- A guard that patrols between two points and chases the player when close.
function Guard.on_start(self)
    self.home = self.entity:world_position()
    self.post = self.home + vector.create(8, 0, 0)
    self.entity:move_to(self.post)
end

function Guard.on_update(self, dt)
    local player = world.find("Player")
    if player and vector.magnitude(player:world_position() - self.entity:world_position()) < 6 then
        self.entity:move_to(player:world_position())
    elseif self.entity:nav_status() == "arrived" then
        -- Turn back at each end.
        self.home, self.post = self.post, self.home
        self.entity:move_to(self.post)
    end
end

return Guard
