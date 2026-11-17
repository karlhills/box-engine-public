local Hello = {}

function Hello.on_start(self)
    print("hello from", self.entity:name())
end

return Hello
