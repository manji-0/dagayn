/**
 * Concrete user service (impl layer).
 * Implements IUserService — cross-package IMPLEMENTS edge toward api/.
 */
public class UserServiceImpl implements IUserService {
    private IRepository repo;

    public UserServiceImpl(IRepository repo) {
        this.repo = repo;
    }

    @Override
    public User create(String name, String email) {
        User user = new User(1, name, email);
        repo.save(user);
        return user;
    }

    @Override
    public User findById(int id) {
        return (User) repo.findById(id);
    }
}
