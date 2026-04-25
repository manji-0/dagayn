/**
 * Generic repository interface (api layer).
 * Abstract, stable — everything depends on this, it depends on nothing.
 */
public interface IRepository {
    Object findById(int id);
    void save(Object item);
    void delete(int id);
}
