/**
 * User service contract (api layer).
 */
public interface IUserService {
    User create(String name, String email);
    User findById(int id);
}
