defmodule Demo.Server do
  use GenServer

  @impl true
  def handle_call(:ping, _from, state) do
    {:reply, :pong, state}
  end

  def elixir_unused_helper do
    1
  end
end
