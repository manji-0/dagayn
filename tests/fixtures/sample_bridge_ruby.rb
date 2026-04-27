# Fixture for bridge detection tests (Ruby).

def run_command
  system("git status")
end

def read_config
  File.read("config.yaml")
end

def write_output
  File.write("output.json", "{}")
end

def open_file
  File.open("data/model.bin", "rb")
end

def load_lib
  Fiddle.dlopen("mylib.so")
end

def read_dynamic(path)
  # Dynamic path — LOW confidence edge
  File.read(path)
end
