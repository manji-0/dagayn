import java.util.HashMap;
import java.util.Map;

/**
 * Abstract in-memory repository base (impl layer).
 * Implements IRepository — cross-package IMPLEMENTS edge toward api/.
 */
public abstract class InMemoryRepository implements IRepository {
    protected Map<Integer, Object> store = new HashMap<>();

    @Override
    public Object findById(int id) {
        return store.get(id);
    }

    @Override
    public void save(Object item) {
        // subclasses provide keyed storage
    }

    @Override
    public void delete(int id) {
        store.remove(id);
    }
}
